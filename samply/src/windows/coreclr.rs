#![allow(unused)]
#![allow(clippy::wildcard_in_or_patterns)]
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    convert::TryInto,
    fs::File,
    io::BufWriter,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use bitflags::bitflags;
use debugid::DebugId;
use fxprof_processed_profile::*;
use serde_json::{json, to_writer, Value};
use uuid::Uuid;

use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::lib_mappings::LibMappingInfo;
use crate::shared::lib_mappings::{LibMappingAdd, LibMappingOp, LibMappingOpQueue};
use crate::shared::process_sample_data::{MarkerSpanOnThread, ProcessSampleData, SimpleMarker};
use crate::shared::recording_props::RecordingProps;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::SampleOrMarker;
use crate::shared::{
    context_switch::{ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData},
    recording_props::{ProcessLaunchProps, RecordingMode},
};
use crate::shared::{
    jit_function_add_marker::JitFunctionAddMarker, marker_file::get_markers,
    process_sample_data::UserTimingMarker, timestamp_converter::TimestampConverter,
};

use etw_reader::{self, etw_types::EventRecord, schema::TypedEvent};
use etw_reader::{
    event_properties_to_string, open_trace,
    parser::{Address, Parser, TryParse},
    print_property,
    schema::SchemaLocator,
    write_property, GUID,
};

use crate::windows::profile_context::{KnownCategory, ProfileContext};

struct SavedMarkerInfo {
    start: Timestamp,
    name: String,
    description: String,
}

pub struct CoreClrContext {
    last_marker_on_thread: HashMap<ThreadHandle, MarkerHandle>,
    gc_markers_on_thread: HashMap<ThreadHandle, HashMap<&'static str, SavedMarkerInfo>>,
}

impl CoreClrContext {
    pub fn new() -> Self {
        Self {
            last_marker_on_thread: HashMap::new(),
            gc_markers_on_thread: HashMap::new(),
        }
    }

    fn remove_last_event_for_thread(&mut self, thread: ThreadHandle) -> Option<MarkerHandle> {
        self.last_marker_on_thread.remove(&thread)
    }

    fn set_last_event_for_thread(&mut self, thread: ThreadHandle, marker: MarkerHandle) {
        self.last_marker_on_thread.insert(thread, marker);
    }

    fn save_gc_marker(
        &mut self,
        thread: ThreadHandle,
        start: Timestamp,
        event: &'static str,
        name: String,
        description: String,
    ) {
        self.gc_markers_on_thread.entry(thread).or_default().insert(
            event,
            SavedMarkerInfo {
                start,
                name,
                description,
            },
        );
    }

    fn remove_gc_marker(&mut self, thread: ThreadHandle, event: &str) -> Option<SavedMarkerInfo> {
        self.gc_markers_on_thread
            .get_mut(&thread)
            .and_then(|m| m.remove(event))
    }
}

bitflags! {
    #[derive(PartialEq, Eq)]
    pub struct CoreClrMethodFlagsMap: u32 {
        const dynamic = 0x1;
        const generic = 0x2;
        const has_shared_generic_code = 0x4;
        const jitted = 0x8;
        const jit_helper = 0x10;
        const profiler_rejected_precompiled_code = 0x20;
        const ready_to_run_rejected_precompiled_code = 0x40;

        // next three bits are the tiered compilation level
        const opttier_bit0 = 0x80;
        const opttier_bit1 = 0x100;
        const opttier_bit2 = 0x200;

        // extent flags/value (hot/cold)
        const extent_bit_0 = 0x10000000; // 0x1 == cold, 0x0 = hot
        const extent_bit_1 = 0x20000000; // always 0 for now looks like
        const extent_bit_2 = 0x40000000;
        const extent_bit_3 = 0x80000000;

        const _ = !0;
    }
    #[derive(PartialEq, Eq)]
    pub struct TieredCompilationSettingsMap: u32 {
        const None = 0x0;
        const QuickJit = 0x1;
        const QuickJitForLoops = 0x2;
        const TieredPGO = 0x4;
        const ReadyToRun = 0x8;
    }
}

// String is type name
#[derive(Debug, Clone)]
pub struct CoreClrGcAllocMarker(pub String, usize);

impl ProfilerMarker for CoreClrGcAllocMarker {
    const MARKER_TYPE_NAME: &'static str = "GC Alloc";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "clrtype": self.0,
            "size": self.1,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![
                MarkerLocation::MarkerChart,
                MarkerLocation::MarkerTable,
                MarkerLocation::TimelineMemory,
            ],
            chart_label: Some("GC Alloc"),
            tooltip_label: Some("GC Alloc: {marker.data.clrtype} ({marker.data.size} bytes)"),
            table_label: Some("GC Alloc"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "clrtype",
                    label: "CLR Type",
                    format: MarkerFieldFormat::String,
                    searchable: true,
                }),
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "size",
                    label: "Size",
                    format: MarkerFieldFormat::Bytes,
                    searchable: false,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "GC Allocation.",
                }),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreClrGcEventMarker(pub String);

impl ProfilerMarker for CoreClrGcEventMarker {
    const MARKER_TYPE_NAME: &'static str = "GC Event";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "event": self.0,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![
                MarkerLocation::MarkerChart,
                MarkerLocation::MarkerTable,
                MarkerLocation::TimelineMemory,
            ],
            chart_label: Some("{marker.data.event}"),
            tooltip_label: Some("{marker.data.event}"),
            table_label: Some("{marker.data.event}"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "event",
                    label: "Event",
                    format: MarkerFieldFormat::String,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Generic GC Event.",
                }),
            ],
        }
    }
}

pub fn coreclr_xperf_args(props: &RecordingProps, recording_mode: &RecordingMode) -> Vec<String> {
    let mut providers = vec![];

    if !props.coreclr {
        return providers;
    }

    let is_attach = match recording_mode {
        RecordingMode::All => true,
        RecordingMode::Pid(_) => true,
        RecordingMode::Launch(_) => false,
    };

    // Enabling all the DotNETRuntime keywords is very expensive. In particular,
    // enabling the NGenKeyword causes info to be generated for every NGen'd method; we should
    // instead use the native PDB info from ModuleLoad events to get this information.
    //
    // Also enabling the rundown keyword causes a bunch of DCStart/DCEnd events to be generated,
    // which is only useful if we're tracing an already running process.
    const CORECLR_GC_KEYWORD: u64 = 0x1; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-garbage-collection-events
    const CORECLR_GC_HANDLE_KEYWORD: u64 = 0x2;
    const CORECLR_BINDER_KEYWORD: u64 = 0x4; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-loader-binder-events
    const CORECLR_LOADER_KEYWORD: u64 = 0x8; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-loader-binder-events
    const CORECLR_JIT_KEYWORD: u64 = 0x10; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-method-events
    const CORECLR_NGEN_KEYWORD: u64 = 0x20; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-method-events
    const CORECLR_INTEROP_KEYWORD: u64 = 0x2000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-interop-events
    const CORECLR_CONTENTION_KEYWORD: u64 = 0x4000;
    const CORECLR_EXCEPTION_KEYWORD: u64 = 0x8000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-exception-events
    const CORECLR_THREADING_KEYWORD: u64 = 0x10000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-thread-events
    const CORECLR_JIT_TO_NATIVE_METHOD_MAP_KEYWORD: u64 = 0x20000;
    const CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_HIGH_KEYWORD: u64 = 0x200000; // https://medium.com/criteo-engineering/build-your-own-net-memory-profiler-in-c-allocations-1-2-9c9f0c86cefd
    const CORECLR_GC_HEAP_AND_TYPE_NAMES: u64 = 0x1000000;
    const CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_LOW_KEYWORD: u64 = 0x2000000;
    const CORECLR_STACK_KEYWORD: u64 = 0x40000000; // https://learn.microsoft.com/en-us/dotnet/framework/performance/stack-etw-event (note: says .NET Framework, but applies to CoreCLR also)
    const CORECLR_COMPILATION_KEYWORD: u64 = 0x1000000000;
    const CORECLR_COMPILATION_DIAGNOSTIC_KEYWORD: u64 = 0x2000000000;
    const CORECLR_TYPE_DIAGNOSTIC_KEYWORD: u64 = 0x8000000000;

    const CORECLR_RUNDOWN_START_KEYWORD: u64 = 0x00000040;

    // if STACK is enabled, then every CoreCLR event will also generate a stack event right afterwards
    let mut info_keywords = CORECLR_LOADER_KEYWORD | CORECLR_STACK_KEYWORD | CORECLR_GC_KEYWORD;
    let mut verbose_keywords = CORECLR_JIT_KEYWORD | CORECLR_NGEN_KEYWORD;
    // if we're attaching, ask for a rundown of method info at the start of collection
    let mut rundown_verbose_keywords = if is_attach {
        CORECLR_LOADER_KEYWORD | CORECLR_JIT_KEYWORD | CORECLR_RUNDOWN_START_KEYWORD
    } else {
        0
    };

    if props.coreclr_allocs {
        info_keywords |= CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_HIGH_KEYWORD
            | CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_LOW_KEYWORD;
    }

    if info_keywords != 0 {
        providers.push(format!(
            "Microsoft-Windows-DotNETRuntime:0x{:x}:4",
            info_keywords
        ));
    }

    if verbose_keywords != 0 {
        // For some reason, we don't get JIT MethodLoad (non-Verbose) in Info level,
        // even though we should. This is OK though, because non-Verbose MethodLoad doesn't
        // include the method string names (we would have to pull it out based on MethodID,
        // and I'm not sure which events include the mapping -- MethodJittingStarted is also
        // verbose).
        providers.push(format!(
            "Microsoft-Windows-DotNETRuntime:0x{:x}:5",
            verbose_keywords
        ));
    }

    if rundown_verbose_keywords != 0 {
        providers.push(format!(
            "Microsoft-Windows-DotNETRuntimeRundown:0x{:x}:5",
            rundown_verbose_keywords
        ));
    }

    //providers.push(format!("Microsoft-Windows-DotNETRuntime"));

    providers
}

pub fn handle_coreclr_event(
    context: &mut ProfileContext,
    s: &TypedEvent,
    parser: &mut Parser,
    timestamp_converter: &TimestampConverter,
) {
    let timestamp_raw = s.timestamp() as u64;
    let timestamp = timestamp_converter.convert_time(timestamp_raw);

    if !context.is_interesting_process(s.process_id(), None, None) {
        return;
    }

    let Some(dotnet_event) = s
        .name()
        .strip_prefix("Microsoft-Windows-DotNETRuntime/")
        .or(s
            .name()
            .strip_prefix("Microsoft-Windows-DotNETRuntimeRundown/"))
    else {
        panic!("Unexpected event {}", s.name())
    };

    let process_id = s.process_id();
    let thread_id = s.thread_id();
    let process_handle = context.get_process_handle(process_id).unwrap();
    let thread_handle = context.get_thread_handle(thread_id).unwrap();

    // TODO -- we may need to use the rundown provider if we trace running processes
    // https://learn.microsoft.com/en-us/dotnet/framework/performance/clr-etw-providers

    // We get DbgID_RSDS for ReadyToRun loaded images, along with PDB files. We also get ModuleLoad events for the same:
    // this means we can ignore the ModuleLoadEvents because we'll get dbginfo already mapped properly when the image
    // is loaded.

    let mut handled = false;

    //eprintln!("event: {} [pid: {} tid: {}] {}", timestamp_raw, s.process_id(), s.thread_id(), dotnet_event);

    // If we get a non-stackwalk event followed by a non-stackwalk event for a given thread,
    // clear out any marker that may have been created to make sure the stackwalk doesn't
    // get attached to the wrong thing.
    if dotnet_event != "CLRStack/CLRStackWalk" {
        context
            .coreclr_context
            .borrow_mut()
            .remove_last_event_for_thread(thread_handle);
    }

    if let Some(method_event) = dotnet_event
        .strip_prefix("CLRMethod/")
        .or(dotnet_event.strip_prefix("CLRMethodRundown/"))
    {
        match method_event {
            // there's MethodDCStart & MethodDCStartVerbose & MethodLoad
            // difference between *Verbose and not, is Verbose includes the names

            "MethodLoadVerbose" | "MethodDCStartVerbose"
            // | "R2RGetEntryPoint" // not sure we need this? R2R methods should be covered by PDB files
            => {
                // R2RGetEntryPoint shares a lot of fields with MethodLoadVerbose
                let is_r2r = method_event == "R2RGetEntryPoint";

                context.ensure_process_jit_info(process_id);
                let Some(process) = context.get_process(process_id) else { return; };

                //let method_id: u64 = parser.parse("MethodID");
                //let clr_instance_id: u32 = parser.parse("ClrInstanceID"); // v1/v2 only

                let method_basename: String = parser.parse("MethodName");
                let method_namespace: String = parser.parse("MethodNamespace");
                let method_signature: String = parser.parse("MethodSignature");

                let method_start_address: Address = if is_r2r { parser.parse("EntryPoint") } else { parser.parse("MethodStartAddress") };
                let method_size: u32 = parser.parse("MethodSize"); // TODO: R2R doesn't have a size?

                // There's a v0, v1, and v2 version of this event. There are rules in `eventtrace.cpp` in the runtime
                // that describe the rules, but basically:
                // - during a first-JIT, only a v1 (not v0 and not v2+) MethodLoad is emitted.
                // - during a re-jit, a v2 event is emitted.
                // - v2 contains a "NativeCodeId" field which will be nonzero in v2. 
                // - the unique key for a method extent is MethodId + MethodCodeId + extent (hot/cold)

                // there's some stuff in MethodFlags -- might be tiered JIT info?
                // also ClrInstanceID -- we probably won't have more than one runtime, but maybe.

                let method_name = format!("{method_basename} [{method_namespace}] \u{2329}{method_signature}\u{232a}");

                let mut process_jit_info = context.get_process_jit_info(process_id);
                let start_address = method_start_address.as_u64();
                let relative_address = process_jit_info.next_relative_address;
                process_jit_info.next_relative_address += method_size;

                // Not that useful for CoreCLR
                //let mh = context.add_thread_marker(s.thread_id(), timestamp, CategoryHandle::OTHER, "JitFunctionAdd", JitFunctionAddMarker(method_name.to_owned()));
                //context.coreclr_context.borrow_mut().set_last_event_for_thread(thread_handle, mh);

                let category = context.get_category(KnownCategory::CoreClrJit);
                let info = LibMappingInfo::new_jit_function(process_jit_info.lib_handle, category.into(), None);
                process_jit_info.jit_mapping_ops.push(timestamp_raw, LibMappingOp::Add(LibMappingAdd {
                    start_avma: start_address,
                    end_avma: start_address + (method_size as u64),
                    relative_address_at_start: relative_address,
                    info
                }));
                process_jit_info.symbols.push(Symbol {
                    address: relative_address,
                    size: Some(method_size),
                    name: method_name,
                });

                handled = true;
            }
            "ModuleLoad" | "ModuleDCStart" |
            "ModuleUnload" | "ModuleDCEnd" => {
                // do we need this for ReadyToRun code?

                //let module_id: u64 = parser.parse("ModuleID");
                //let assembly_id: u64 = parser.parse("AssemblyId");
                //let managed_pdb_signature: u?? = parser.parse("ManagedPdbSignature");
                //let managed_pdb_age: u?? = parser.parse("ManagedPdbAge");
                //let managed_pdb_path: String = parser.parse("ManagedPdbPath");
                //let native_pdb_signature: u?? = parser.parse("NativePdbSignature");
                //let native_pdb_age: u?? = parser.parse("NativePdbAge");
                //let native_pdb_path: String = parser.parse("NativePdbPath");
                handled = true;
            }
            _ => {
                // don't care about any other CLRMethod events
                handled = true;
            }
        }
    } else if dotnet_event == "Type/BulkType" {
        //         <template tid="BulkType">
        // <data name="Count" inType="win:UInt32"    />
        // <data name="ClrInstanceID" inType="win:UInt16" />
        // <struct name="Values" count="Count" >
        // <data name="TypeID" inType="win:UInt64" outType="win:HexInt64" />
        // <data name="ModuleID" inType="win:UInt64" outType="win:HexInt64" />
        // <data name="TypeNameID" inType="win:UInt32" />
        // <data name="Flags" inType="win:UInt32" map="TypeFlagsMap"/>
        // <data name="CorElementType"  inType="win:UInt8" />
        // <data name="Name" inType="win:UnicodeString" />
        // <data name="TypeParameterCount" inType="win:UInt32" />
        // <data name="TypeParameters"  count="TypeParameterCount"  inType="win:UInt64" outType="win:HexInt64" />
        // </struct>
        // <UserData>
        // <Type xmlns="myNs">
        // <Count> %1 </Count>
        // <ClrInstanceID> %2 </ClrInstanceID>
        // </Type>
        // </UserData>
        //let count: u32 = parser.parse("Count");

        // uint32 + uint16 at the front (Count and ClrInstanceID), then struct of values. We don't need a Vec<u8> copy.
        //let values: Vec<u8> = parser.parse("Values");
        //let values = &s.user_buffer()[6..];

        //eprintln!("Type/BulkType count: {} user_buffer size: {} values len: {}", count, s.user_buffer().len(), values.len());
    } else if dotnet_event == "CLRStack/CLRStackWalk" {
        // If the STACK keyword is enabled, we get a CLRStackWalk following each CLR event that supports stacks. Not every event
        // does. The info about which does and doesn't is here: https://github.com/dotnet/runtime/blob/main/src/coreclr/vm/ClrEtwAllMeta.lst
        // Current dotnet (8.0.x) seems to have a bug where `MethodJitMemoryAllocatedForCode` events will fire a stackwalk,
        // but the event itself doesn't end up in the trace. (https://github.com/dotnet/runtime/issues/102004)

        // if we don't have anything to attach this stack to, just skip it
        let Some(marker) = context
            .get_coreclr_context()
            .remove_last_event_for_thread(thread_handle)
        else {
            return;
        };

        // "Stack" is explicitly declared as length 2 in the manifest, so the first two addresses are in here, rest
        // are in user data buffer.
        let first_addresses: Vec<u8> = parser.parse("Stack");
        let stack: Vec<StackFrame> = first_addresses
            .chunks_exact(8)
            .chain(parser.buffer.chunks_exact(8))
            .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
            .enumerate()
            .map(|(i, addr)| {
                if i == 0 {
                    StackFrame::InstructionPointer(addr, context.stack_mode_for_address(addr))
                } else {
                    StackFrame::ReturnAddress(addr, context.stack_mode_for_address(addr))
                }
            })
            .collect();
        let stack_index = context.unresolved_stack_handle_for_stack(&stack);

        //eprintln!("event: StackWalk stack: {:?}", stack);

        // Note: we don't add these as actual samples, and instead just attach them to the marker.
        // If we added them as samples, it would throw off the profile counting, because they arrive
        // in between regular interval samples. In the future, maybe we can support fractional samples
        // somehow (fractional weight), but for now, we just attach them to the marker.

        context
            .get_process_mut(process_id)
            .unwrap()
            .unresolved_samples
            .add_sample_or_marker(
                thread_handle,
                timestamp,
                timestamp_raw,
                stack_index,
                SampleOrMarker::MarkerHandle(marker),
            );
        handled = true;
    } else if let Some(gc_event) = dotnet_event.strip_prefix("GarbageCollection/") {
        let gc_category = context.get_category(KnownCategory::CoreClrGc);
        match gc_event {
            "GCSampledObjectAllocation" => {
                // If High/Low flags are set, then we get one of these for every alloc. Otherwise only
                // when a threshold is hit. (100kb) The count and size are aggregates in that case.
                let type_id: u64 = parser.parse("TypeID"); // TODO: convert to str, with bulk type data
                                                           //let address: u64 = parser.parse("Address");
                let object_count: u32 = parser.parse("ObjectCountForTypeSample");
                let total_size: u64 = parser.parse("TotalSizeForTypeSample");

                let mh = context.profile.borrow_mut().add_marker(
                    thread_handle,
                    gc_category,
                    "GC Alloc",
                    CoreClrGcAllocMarker(format!("0x{:x}", type_id), total_size as usize),
                    MarkerTiming::Instant(timestamp),
                );
                context
                    .coreclr_context
                    .borrow_mut()
                    .set_last_event_for_thread(thread_handle, mh);
                handled = true;
            }
            "Triggered" => {
                let reason: u32 = parser.parse("Reason");

                let reason_str = match reason {
                    0x0 => "AllocSmall",
                    0x1 => "Induced",
                    0x2 => "LowMemory",
                    0x3 => "Empty",
                    0x4 => "AllocLarge",
                    0x5 => "OutOfSpaceSmallObjectHeap",
                    0x6 => "OutOfSpaceLargeObjectHeap",
                    0x7 => "nducedNoForce",
                    0x8 => "Stress",
                    0x9 => "InducedLowMemory",
                    _ => {
                        eprintln!("Unknown CLR GC Triggered reason: {}", reason);
                        "Unknown"
                    }
                };

                let mh = context.profile.borrow_mut().add_marker(
                    thread_handle,
                    gc_category,
                    "GC Trigger",
                    CoreClrGcEventMarker(format!("GC Trigger: {}", reason_str)),
                    MarkerTiming::Instant(timestamp),
                );
                context
                    .coreclr_context
                    .borrow_mut()
                    .set_last_event_for_thread(thread_handle, mh);
                handled = true;
            }
            "GCSuspendEEBegin" => {
                // Reason, Count
                let count: u32 = parser.parse("Count");
                let reason: u32 = parser.parse("Reason");

                let reason_str = match reason {
                    0x0 => "Other",
                    0x1 => "GC",
                    0x2 => "AppDomain shutdown",
                    0x3 => "Code pitching",
                    0x4 => "Shutdown",
                    0x5 => "Debugger",
                    0x6 => "GC Prep",
                    0x7 => "Debugger sweep",
                    _ => {
                        eprintln!("Unknown CLR GCSuspendEEBegin reason: {}", reason);
                        "Unknown reason"
                    }
                };

                context.coreclr_context.borrow_mut().save_gc_marker(
                    thread_handle,
                    timestamp,
                    "GCSuspendEE",
                    "GC Suspended Thread".to_owned(),
                    format!("Suspended: {}", reason_str),
                );
                handled = true;
            }
            "GCSuspendEEEnd" | "GCRestartEEBegin" => {
                // don't care -- we only care about SuspendBegin and RestartEnd
                handled = true;
            }
            "GCRestartEEEnd" => {
                if let Some(info) = context
                    .coreclr_context
                    .borrow_mut()
                    .remove_gc_marker(thread_handle, "GCSuspendEE")
                {
                    context.profile.borrow_mut().add_marker(
                        thread_handle,
                        gc_category,
                        &info.name,
                        CoreClrGcEventMarker(info.description),
                        MarkerTiming::Interval(info.start, timestamp),
                    );
                }
                handled = true;
            }
            "win:Start" => {
                let count: u32 = parser.parse("Count");
                let depth: u32 = parser.parse("Depth");
                let reason: u32 = parser.parse("Reason");
                let gc_type: u32 = parser.parse("Type");

                let reason_str = match reason {
                    0x0 => "Small object heap allocation",
                    0x1 => "Induced",
                    0x2 => "Low memory",
                    0x3 => "Empty",
                    0x4 => "Large object heap allocation",
                    0x5 => "Out of space (for small object heap)",
                    0x6 => "Out of space (for large object heap)",
                    0x7 => "Induced but not forced as blocking",
                    _ => {
                        eprintln!("Unknown CLR GCStart reason: {}", reason);
                        "Unknown reason"
                    }
                };

                let gc_type_str = match gc_type {
                    0x0 => "Blocking GC",
                    0x1 => "Background GC",
                    0x2 => "Blocking GC during background GC",
                    _ => {
                        eprintln!("Unknown CLR GCStart type: {}", gc_type);
                        "Unknown type"
                    }
                };

                // TODO: use gc_type_str as the name
                context.coreclr_context.borrow_mut().save_gc_marker(
                    thread_handle,
                    timestamp,
                    "GC",
                    "GC".to_owned(),
                    format!(
                        "{}: {} (GC #{}, gen{})",
                        gc_type_str, reason_str, count, depth
                    ),
                );
                handled = true;
            }
            "win:Stop" => {
                //let count: u32 = parser.parse("Count");
                //let depth: u32 = parser.parse("Depth");
                if let Some(info) = context
                    .coreclr_context
                    .borrow_mut()
                    .remove_gc_marker(thread_handle, "GC")
                {
                    context.profile.borrow_mut().add_marker(
                        thread_handle,
                        gc_category,
                        &info.name,
                        CoreClrGcEventMarker(info.description),
                        MarkerTiming::Interval(info.start, timestamp),
                    );
                }
                handled = true;
            }
            "SetGCHandle" => {
                // TODO
            }
            "DestroyGCHandle" => {
                // TODO
            }
            "GCFinalizersBegin" | "GCFinalizersEnd" | "FinalizeObject" => {
                // TODO: create an interval
                handled = true;
            }
            "GCCreateSegment" | "GCFreeSegment" | "GCDynamicEvent" | "GCHeapStats" | _ => {
                // don't care
                handled = true;
            }
        }
    } else if dotnet_event.starts_with("CLRRuntimeInformation/") {
        handled = true;
    } else if dotnet_event.starts_with("CLRLoader/") {
        // AppDomain, Assembly, Module Load/Unload
        handled = true;
    }

    if !handled {
        //if dotnet_event.contains("GarbageCollection") { return }
        //if dotnet_event.contains("/Thread") { return }
        //if dotnet_event.contains("Type/BulkType") { return }
        let text = event_properties_to_string(s, parser, None);
        let mh = context.add_thread_marker(
            s.thread_id(),
            timestamp,
            context.get_category(KnownCategory::Unknown),
            s.name().split_once('/').unwrap().1,
            SimpleMarker(text),
        );

        context
            .coreclr_context
            .borrow_mut()
            .set_last_event_for_thread(thread_handle, mh);
        //eprintln!("Unhandled .NET event: tid {} {} {:?}", s.thread_id(), dotnet_event, mh.unwrap());
    }
}
