use std::{collections::HashMap, convert::TryInto, fmt::Display};

use bitflags::bitflags;
use fxprof_processed_profile::*;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use serde_json::json;

use etw_reader::{self, schema::TypedEvent};
use etw_reader::{
    event_properties_to_string,
    parser::{Parser, TryParse},
};

use crate::shared::process_sample_data::SimpleMarker;
use crate::shared::recording_props::{CoreClrProfileProps, ProfileCreationProps};
use crate::windows::profile_context::{KnownCategory, ProfileContext};

use super::elevated_helper::ElevatedRecordingProps;

struct SavedMarkerInfo {
    start_timestamp_raw: u64,
    name: String,
    description: String,
}

pub struct CoreClrContext {
    props: CoreClrProfileProps,
    last_marker_on_thread: HashMap<u32, MarkerHandle>,
    gc_markers_on_thread: HashMap<u32, HashMap<&'static str, SavedMarkerInfo>>,
    unknown_event_markers: bool,
}

impl CoreClrContext {
    pub fn new(profile_creation_props: ProfileCreationProps) -> Self {
        Self {
            props: profile_creation_props.coreclr,
            last_marker_on_thread: HashMap::new(),
            gc_markers_on_thread: HashMap::new(),
            unknown_event_markers: profile_creation_props.unknown_event_markers,
        }
    }

    fn remove_last_event_for_thread(&mut self, tid: u32) -> Option<MarkerHandle> {
        self.last_marker_on_thread.remove(&tid)
    }

    fn set_last_event_for_thread(&mut self, tid: u32, marker: MarkerHandle) {
        self.last_marker_on_thread.insert(tid, marker);
    }

    fn save_gc_marker(
        &mut self,
        tid: u32,
        start_timestamp_raw: u64,
        event: &'static str,
        name: String,
        description: String,
    ) {
        self.gc_markers_on_thread.entry(tid).or_default().insert(
            event,
            SavedMarkerInfo {
                start_timestamp_raw,
                name,
                description,
            },
        );
    }

    fn remove_gc_marker(&mut self, tid: u32, event: &str) -> Option<SavedMarkerInfo> {
        self.gc_markers_on_thread
            .get_mut(&tid)
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

#[allow(unused)]
mod constants {
    pub const CORECLR_GC_KEYWORD: u64 = 0x1; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-garbage-collection-events
    pub const CORECLR_GC_HANDLE_KEYWORD: u64 = 0x2;
    pub const CORECLR_BINDER_KEYWORD: u64 = 0x4; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-loader-binder-events
    pub const CORECLR_LOADER_KEYWORD: u64 = 0x8; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-loader-binder-events
    pub const CORECLR_JIT_KEYWORD: u64 = 0x10; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-method-events
    pub const CORECLR_NGEN_KEYWORD: u64 = 0x20; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-method-events
    pub const CORECLR_RUNDOWN_START_KEYWORD: u64 = 0x00000040;
    pub const CORECLR_INTEROP_KEYWORD: u64 = 0x2000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-interop-events
    pub const CORECLR_CONTENTION_KEYWORD: u64 = 0x4000;
    pub const CORECLR_EXCEPTION_KEYWORD: u64 = 0x8000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-exception-events
    pub const CORECLR_THREADING_KEYWORD: u64 = 0x10000; // https://learn.microsoft.com/en-us/dotnet/fundamentals/diagnostics/runtime-thread-events
    pub const CORECLR_JIT_TO_NATIVE_METHOD_MAP_KEYWORD: u64 = 0x20000;
    pub const CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_HIGH_KEYWORD: u64 = 0x200000; // https://medium.com/criteo-engineering/build-your-own-net-memory-profiler-in-c-allocations-1-2-9c9f0c86cefd
    pub const CORECLR_GC_HEAP_AND_TYPE_NAMES: u64 = 0x1000000;
    pub const CORECLR_GC_SAMPLED_OBJECT_ALLOCATION_LOW_KEYWORD: u64 = 0x2000000;
    pub const CORECLR_STACK_KEYWORD: u64 = 0x40000000; // https://learn.microsoft.com/en-us/dotnet/framework/performance/stack-etw-event (note: says .NET Framework, but applies to CoreCLR also)
    pub const CORECLR_COMPILATION_KEYWORD: u64 = 0x1000000000;
    pub const CORECLR_COMPILATION_DIAGNOSTIC_KEYWORD: u64 = 0x2000000000;
    pub const CORECLR_TYPE_DIAGNOSTIC_KEYWORD: u64 = 0x8000000000;
}

#[derive(Debug, Clone, FromPrimitive)]
enum GcReason {
    AllocSmall = 0,
    Induced,
    LowMemory,
    Empty,
    AllocLarge,
    OutOfSpaceSmallObjectHeap,
    OutOfSpaceLargeObjectHeap,
    InducedNoForce,
    Stress,
    InducedLowMemory,
}

impl Display for GcReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GcReason::AllocSmall => f.write_str("Small object heap allocation"),
            GcReason::Induced => f.write_str("Induced"),
            GcReason::LowMemory => f.write_str("Low memory"),
            GcReason::Empty => f.write_str("Empty"),
            GcReason::AllocLarge => f.write_str("Large object heap allocation"),
            GcReason::OutOfSpaceSmallObjectHeap => {
                f.write_str("Out of space (for small object heap)")
            }
            GcReason::OutOfSpaceLargeObjectHeap => {
                f.write_str("Out of space (for large object heap)")
            }
            GcReason::InducedNoForce => f.write_str("Induced but not forced as blocking"),
            GcReason::Stress => f.write_str("Stress"),
            GcReason::InducedLowMemory => f.write_str("Induced low memory"),
        }
    }
}

#[derive(Debug, Clone, FromPrimitive)]
enum GcSuspendEeReason {
    Other = 0,
    GC,
    AppDomainShutdown,
    CodePitching,
    Shutdown,
    Debugger,
    GcPrep,
    DebuggerSweep,
}

impl Display for GcSuspendEeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GcSuspendEeReason::Other => f.write_str("Other"),
            GcSuspendEeReason::GC => f.write_str("GC"),
            GcSuspendEeReason::AppDomainShutdown => f.write_str("AppDomain shutdown"),
            GcSuspendEeReason::CodePitching => f.write_str("Code pitching"),
            GcSuspendEeReason::Shutdown => f.write_str("Shutdown"),
            GcSuspendEeReason::Debugger => f.write_str("Debugger"),
            GcSuspendEeReason::GcPrep => f.write_str("GC prep"),
            GcSuspendEeReason::DebuggerSweep => f.write_str("Debugger sweep"),
        }
    }
}

#[derive(Debug, Clone, FromPrimitive)]
enum GcType {
    Blocking,
    Background,
    BlockingDuringBackground,
}

impl Display for GcType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GcType::Blocking => f.write_str("Blocking GC"),
            GcType::Background => f.write_str("Background GC"),
            GcType::BlockingDuringBackground => f.write_str("Blocking GC during background GC"),
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayUnknownIfNone<'a, T>(pub &'a Option<T>);

impl<'a, T: Display> Display for DisplayUnknownIfNone<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("Unknown"),
        }
    }
}

pub fn coreclr_xperf_args(props: &ElevatedRecordingProps) -> Vec<String> {
    let mut providers = vec![];

    if !props.coreclr.any_enabled() {
        return providers;
    }

    // Enabling all the DotNETRuntime keywords is very expensive. In particular,
    // enabling the NGenKeyword causes info to be generated for every NGen'd method; we should
    // instead use the native PDB info from ModuleLoad events to get this information.
    //
    // Also enabling the rundown keyword causes a bunch of DCStart/DCEnd events to be generated,
    // which is only useful if we're tracing an already running process.
    // if STACK is enabled, then every CoreCLR event will also generate a stack event right afterwards
    use constants::*;
    let mut info_keywords = CORECLR_LOADER_KEYWORD;
    if props.coreclr.event_stacks {
        info_keywords |= CORECLR_STACK_KEYWORD;
    }
    if props.coreclr.gc_markers || props.coreclr.gc_suspensions || props.coreclr.gc_detailed_allocs
    {
        info_keywords |= CORECLR_GC_KEYWORD;
    }

    let verbose_keywords = CORECLR_JIT_KEYWORD | CORECLR_NGEN_KEYWORD;

    // if we're attaching, ask for a rundown of method info at the start of collection
    let rundown_verbose_keywords = if props.is_attach {
        CORECLR_LOADER_KEYWORD | CORECLR_JIT_KEYWORD | CORECLR_RUNDOWN_START_KEYWORD
    } else {
        0
    };

    if props.coreclr.gc_detailed_allocs {
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
    coreclr_context: &mut CoreClrContext,
    s: &TypedEvent,
    parser: &mut Parser,
) {
    let (gc_markers, gc_suspensions, gc_allocs, event_stacks) = (
        coreclr_context.props.gc_markers,
        coreclr_context.props.gc_suspensions,
        coreclr_context.props.gc_detailed_allocs,
        coreclr_context.props.event_stacks,
    );

    if !context.is_interesting_process(s.process_id(), None, None) {
        return;
    }

    let timestamp_raw = s.timestamp() as u64;

    let mut name_parts = s.name().splitn(3, '/');
    let provider = name_parts.next().unwrap();
    let task = name_parts.next().unwrap();
    let opcode = name_parts.next().unwrap();

    match provider {
        "Microsoft-Windows-DotNETRuntime" | "Microsoft-Windows-DotNETRuntimeRundown" => {}
        _ => {
            panic!("Unexpected event {}", s.name())
        }
    }

    let pid = s.process_id();
    let tid = s.thread_id();

    // TODO -- we may need to use the rundown provider if we trace running processes
    // https://learn.microsoft.com/en-us/dotnet/framework/performance/clr-etw-providers

    // We get DbgID_RSDS for ReadyToRun loaded images, along with PDB files. We also get ModuleLoad events for the same:
    // this means we can ignore the ModuleLoadEvents because we'll get dbginfo already mapped properly when the image
    // is loaded.

    let mut handled = false;

    //eprintln!("event: {} [pid: {} tid: {}] {}", timestamp_raw, s.pid(), s.tid(), dotnet_event);

    // If we get a non-stackwalk event followed by a non-stackwalk event for a given thread,
    // clear out any marker that may have been created to make sure the stackwalk doesn't
    // get attached to the wrong thing.
    if (task, opcode) != ("CLRStack", "CLRStackWalk") {
        coreclr_context.remove_last_event_for_thread(tid);
    }

    match (task, opcode) {
        ("CLRMethod" | "CLRMethodRundown", method_event) => {
            match method_event {
            // there's MethodDCStart & MethodDCStartVerbose & MethodLoad
            // difference between *Verbose and not, is Verbose includes the names

            "MethodLoadVerbose" | "MethodDCStartVerbose"
            // | "R2RGetEntryPoint" // not sure we need this? R2R methods should be covered by PDB files
            => {
                // R2RGetEntryPoint shares a lot of fields with MethodLoadVerbose
                let is_r2r = method_event == "R2RGetEntryPoint";

                //let method_id: u64 = parser.parse("MethodID");
                //let clr_instance_id: u32 = parser.parse("ClrInstanceID"); // v1/v2 only

                let method_basename: String = parser.parse("MethodName");
                let method_namespace: String = parser.parse("MethodNamespace");
                let method_signature: String = parser.parse("MethodSignature");

                let method_start_address: u64 = if is_r2r { parser.parse("EntryPoint") } else { parser.parse("MethodStartAddress") };
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

                context.handle_coreclr_method_load(timestamp_raw, pid, method_name, method_start_address, method_size);
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
        }
        ("Type", "BulkType") => {
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
        }
        ("CLRStack", "CLRStackWalk") => {
            // If the STACK keyword is enabled, we get a CLRStackWalk following each CLR event that supports stacks. Not every event
            // does. The info about which does and doesn't is here: https://github.com/dotnet/runtime/blob/main/src/coreclr/vm/ClrEtwAllMeta.lst
            // Current dotnet (8.0.x) seems to have a bug where `MethodJitMemoryAllocatedForCode` events will fire a stackwalk,
            // but the event itself doesn't end up in the trace. (https://github.com/dotnet/runtime/issues/102004)
            if !event_stacks {
                return;
            }

            // if we don't have anything to attach this stack to, just skip it
            let Some(marker) = coreclr_context.remove_last_event_for_thread(tid) else {
                return;
            };

            // "Stack" is explicitly declared as length 2 in the manifest, so the first two addresses are in here, rest
            // are in user data buffer.
            let first_addresses: Vec<u8> = parser.parse("Stack");
            let address_iter = first_addresses
                .chunks_exact(8)
                .chain(parser.buffer.chunks_exact(8))
                .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()));

            context.handle_coreclr_stack(timestamp_raw, pid, tid, address_iter, marker);
            handled = true;
        }
        ("GarbageCollection", gc_event) => {
            match gc_event {
                "GCSampledObjectAllocation" => {
                    if !gc_allocs {
                        return;
                    }

                    // If High/Low flags are set, then we get one of these for every alloc. Otherwise only
                    // when a threshold is hit. (100kb) The count and size are aggregates in that case.
                    let type_id: u64 = parser.parse("TypeID"); // TODO: convert to str, with bulk type data
                                                               //let address: u64 = parser.parse("Address");
                    let _object_count: u32 = parser.parse("ObjectCountForTypeSample");
                    let total_size: u64 = parser.parse("TotalSizeForTypeSample");

                    let mh = context.add_thread_instant_marker(
                        timestamp_raw,
                        tid,
                        KnownCategory::CoreClrGc,
                        "GC Alloc",
                        CoreClrGcAllocMarker(format!("0x{:x}", type_id), total_size as usize),
                    );
                    coreclr_context.set_last_event_for_thread(tid, mh);
                    handled = true;
                }
                "Triggered" => {
                    if !gc_markers {
                        return;
                    }

                    let reason: u32 = parser.parse("Reason");
                    let reason = GcReason::from_u32(reason).or_else(|| {
                        eprintln!("Unknown CLR GC Triggered reason: {}", reason);
                        None
                    });

                    let mh = context.add_thread_instant_marker(
                        timestamp_raw,
                        tid,
                        KnownCategory::CoreClrGc,
                        "GC Trigger",
                        CoreClrGcEventMarker(format!(
                            "GC Trigger: {}",
                            DisplayUnknownIfNone(&reason)
                        )),
                    );
                    coreclr_context.set_last_event_for_thread(tid, mh);
                    handled = true;
                }
                "GCSuspendEEBegin" => {
                    if !gc_suspensions {
                        return;
                    }

                    // Reason, Count
                    let _count: u32 = parser.parse("Count");
                    let reason: u32 = parser.parse("Reason");

                    let reason = GcSuspendEeReason::from_u32(reason).or_else(|| {
                        eprintln!("Unknown CLR GCSuspendEEBegin reason: {}", reason);
                        None
                    });

                    coreclr_context.save_gc_marker(
                        tid,
                        timestamp_raw,
                        "GCSuspendEE",
                        "GC Suspended Thread".to_owned(),
                        format!("Suspended: {}", DisplayUnknownIfNone(&reason)),
                    );
                    handled = true;
                }
                "GCSuspendEEEnd" | "GCRestartEEBegin" => {
                    // don't care -- we only care about SuspendBegin and RestartEnd
                    handled = true;
                }
                "GCRestartEEEnd" => {
                    if !gc_suspensions {
                        return;
                    }

                    if let Some(info) = coreclr_context.remove_gc_marker(tid, "GCSuspendEE") {
                        context.add_thread_interval_marker(
                            info.start_timestamp_raw,
                            timestamp_raw,
                            tid,
                            KnownCategory::CoreClrGc,
                            &info.name,
                            CoreClrGcEventMarker(info.description),
                        );
                    }
                    handled = true;
                }
                "win:Start" => {
                    if !gc_markers {
                        return;
                    }

                    let count: u32 = parser.parse("Count");
                    let depth: u32 = parser.parse("Depth");
                    let reason: u32 = parser.parse("Reason");
                    let gc_type: u32 = parser.parse("Type");

                    let reason = GcReason::from_u32(reason).or_else(|| {
                        eprintln!("Unknown CLR GCStart reason: {}", reason);
                        None
                    });

                    let gc_type = GcType::from_u32(gc_type).or_else(|| {
                        eprintln!("Unknown CLR GCStart type: {}", gc_type);
                        None
                    });

                    // TODO: use gc_type_str as the name
                    coreclr_context.save_gc_marker(
                        tid,
                        timestamp_raw,
                        "GC",
                        "GC".to_owned(),
                        format!(
                            "{}: {} (GC #{}, gen{})",
                            DisplayUnknownIfNone(&gc_type),
                            DisplayUnknownIfNone(&reason),
                            count,
                            depth
                        ),
                    );
                    handled = true;
                }
                "win:Stop" => {
                    if !gc_markers {
                        return;
                    }

                    //let count: u32 = parser.parse("Count");
                    //let depth: u32 = parser.parse("Depth");
                    if let Some(info) = coreclr_context.remove_gc_marker(tid, "GC") {
                        context.add_thread_interval_marker(
                            info.start_timestamp_raw,
                            timestamp_raw,
                            tid,
                            KnownCategory::CoreClrGc,
                            &info.name,
                            CoreClrGcEventMarker(info.description),
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
                "GCCreateSegment" | "GCFreeSegment" | "GCDynamicEvent" | "GCHeapStats" => {
                    // don't care
                    handled = true;
                }
                _ => {
                    // don't care
                    handled = true;
                }
            }
        }
        ("CLRRuntimeInformation", _) => {
            handled = true;
        }
        ("CLRLoader", _) => {
            // AppDomain, Assembly, Module Load/Unload
            handled = true;
        }
        _ => {}
    }

    if !handled && coreclr_context.unknown_event_markers {
        let text = event_properties_to_string(s, parser, None);
        let marker_handle = context.add_thread_instant_marker(
            timestamp_raw,
            tid,
            KnownCategory::Unknown,
            s.name().split_once('/').unwrap().1,
            SimpleMarker(text),
        );

        coreclr_context.set_last_event_for_thread(tid, marker_handle);
    }
}
