use std::{collections::{hash_map::Entry, HashMap, HashSet, VecDeque}, convert::TryInto, fs::File, io::BufWriter, path::Path, sync::Arc, time::{Duration, Instant, SystemTime}};

use serde_json::{json, to_writer, Value};
use fxprof_processed_profile::{CategoryColor, CategoryHandle, CategoryPairHandle, CounterHandle, CpuDelta, debugid, FrameFlags, FrameInfo, LibraryHandle, LibraryInfo, MarkerDynamicField, MarkerFieldFormat, MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerTiming, ProcessHandle, Profile, ProfilerMarker, ReferenceTimestamp, SamplingInterval, Symbol, SymbolTable, ThreadHandle, Timestamp};
use debugid::DebugId;
use bitflags::bitflags;
use uuid::Uuid;

use crate::shared::lib_mappings::{LibMappingAdd, LibMappingOp, LibMappingOpQueue};
use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::lib_mappings::LibMappingInfo;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::process_sample_data::{MarkerSpanOnThread, ProcessSampleData, SimpleMarker};
use crate::shared::context_switch::{ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData};

use crate::shared::{jit_function_add_marker::JitFunctionAddMarker, marker_file::get_markers, process_sample_data::UserTimingMarker, timestamp_converter::TimestampConverter};

use super::etw_reader;
use super::etw_reader::{GUID, open_trace, parser::{Address, Parser, TryParse}, print_property, schema::SchemaLocator, write_property, event_properties_to_string};
use super::*;

use super::ProfileContext;

pub fn profile_pid_from_etl_file(
    context: &mut ProfileContext,
    etl_file: &Path,
) {
    let profile_start_instant = Timestamp::from_nanos_since_reference(0);

    let arch = &context.arch;
    let is_x86 = arch == "x86" || arch == "x86_64";
    let is_arm64 = arch == "arm64";

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut kernel_pending_libraries: HashMap<u64, LibraryInfo> = HashMap::new();

    let mut libs: HashMap<(u32, u64), (String, u32, u32)> = HashMap::new();

    let processing_start_timestamp = Instant::now();

    //let merge_threads = false; // --merge-threads? (merge samples from all interesting apps into a single thread)
    let include_idle = false; //pargs.contains("--idle");
    let demand_zero_faults = false; //pargs.contains("--demand-zero-faults");
    let marker_file: Option<String> = None; //pargs.opt_value_from_str("--marker-file").unwrap();
    let marker_prefix: Option<String> = None; //pargs.opt_value_from_str("--filter-by-marker-prefix").unwrap();

    let user_category: CategoryPairHandle = context.profile.borrow_mut().add_category("User", fxprof_processed_profile::CategoryColor::Yellow).into();
    let kernel_category: CategoryPairHandle = context.profile.borrow_mut().add_category("Kernel", fxprof_processed_profile::CategoryColor::Orange).into();

    let mut jit_category_manager = JitCategoryManager::new();
    let mut context_switch_handler = ContextSwitchHandler::new(122100);

    let mut sample_count = 0;
    let mut stack_sample_count = 0;
    let mut dropped_sample_count = 0;
    let mut timer_resolution: u32 = 0; // Resolution of the hardware timer, in units of 100 nanoseconds.
    let mut event_count = 0;
    let mut gpu_thread = None;
    let mut jscript_symbols: HashMap<u32, ProcessJitInfo> = HashMap::new();
    let mut jscript_sources: HashMap<u64, String> = HashMap::new();

    // Make a dummy TimestampConverter. Once we've parsed the header, this will have correct values.
    let mut timestamp_converter = TimestampConverter {
        reference_raw: 0,
        raw_to_ns_factor: 1,
    };
    let mut event_timestamps_are_qpc = false;

    let mut categories = HashMap::<String, CategoryHandle>::new();
    let result = open_trace(&etl_file, |e| {
        event_count += 1;
        let s = schema_locator.event_schema(e);

        if let Ok(s) = s {
            let mut parser = Parser::create(&s);
            let timestamp_raw = e.EventHeader.TimeStamp as u64;
            let timestamp = timestamp_converter.convert_time(timestamp_raw);

            //eprintln!("{}", s.name());
            match s.name() {
                "MSNT_SystemTrace/EventTrace/Header" => {
                    timer_resolution = parser.parse("TimerResolution");
                    let perf_freq: u64 = parser.parse("PerfFreq");
                    let clock_type: u32 = parser.parse("ReservedFlags");
                    if clock_type != 1 {
                        println!("WARNING: QPC not used as clock");
                        event_timestamps_are_qpc = false;
                    } else {
                        event_timestamps_are_qpc = true;
                    }
                    let events_lost: u32 = parser.parse("EventsLost");
                    if events_lost != 0 {
                        println!("WARNING: {} events lost", events_lost);
                    }

                    timestamp_converter = TimestampConverter {
                        reference_raw: e.EventHeader.TimeStamp as u64,
                        raw_to_ns_factor: 1000 * 1000 * 1000 / perf_freq,
                    };

                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        print_property(&mut parser, &property, false);
                    }
                }
                "MSNT_SystemTrace/PerfInfo/CollectionStart" => {
                    let interval_raw: u32 = parser.parse("NewInterval");
                    let interval_nanos = interval_raw as u64 * 100;
                    let interval = SamplingInterval::from_nanos(interval_nanos);
                    println!("Sample rate {}ms", interval.as_secs_f64() * 1000.);
                    context.profile.borrow_mut().set_interval(interval);
                    context_switch_handler = ContextSwitchHandler::new(interval_raw as u64);
                }
                "MSNT_SystemTrace/Thread/SetName" => {
                    let thread_id: u32 = parser.parse("ThreadId");
                    let thread_name: String = parser.parse("ThreadName");

                    if !thread_name.is_empty() {
                        context.set_thread_name(thread_id, &thread_name);
                    }
                }
                "MSNT_SystemTrace/Thread/Start" |
                "MSNT_SystemTrace/Thread/DCStart" => {
                    let thread_id: u32 = parser.parse("TThreadId");
                    let process_id: u32 = parser.parse("ProcessId");

                    if !context.is_interesting_process(process_id, None, None) {
                        return;
                    }

                    // if there's an existing thread, remove it, assume we dropped an end thread event
                    context.remove_thread(thread_id, Some(timestamp));
                    context.add_thread(process_id, thread_id, timestamp);

                    let thread_name: Option<String> = parser.try_parse("ThreadName").ok();
                    if let Some(thread_name) = thread_name {
                        if !thread_name.is_empty() {
                            context.set_thread_name(thread_id, &thread_name);
                        }
                    }
                }
                "MSNT_SystemTrace/Thread/End" |
                "MSNT_SystemTrace/Thread/DCEnd" => {
                    let thread_id: u32 = parser.parse("TThreadId");

                    context.remove_thread(thread_id, Some(timestamp));
                }
                "MSNT_SystemTrace/Process/Start" |
                "MSNT_SystemTrace/Process/DCStart" => {
                    let process_id: u32 = parser.parse("ProcessId");
                    let parent_id: u32 = parser.parse("ParentId");
                    let image_file_name: String = parser.parse("ImageFileName");

                    // note: the event's e.EventHeader.process_id here is the parent (i.e. the process that spawned
                    // a new one. The process_id in ProcessId is the new process id.

                    if !context.is_interesting_process(process_id, Some(parent_id), Some(&image_file_name)) {
                        return;
                    }

                    context.add_process(process_id, parent_id, &context.map_device_path(&image_file_name), timestamp);
                }
                "MSNT_SystemTrace/Process/End" |
                "MSNT_SystemTrace/Process/DCEnd" => {
                    let process_id: u32 = parser.parse("ProcessId");

                    //context.end_process(...);
                }
                "MSNT_SystemTrace/StackWalk/Stack" => {
                    let thread_id: u32 = parser.parse("StackThread");
                    let process_id: u32 = parser.parse("StackProcess");
                    // The EventTimeStamp here indicates thea ccurate time the stack was collected, and
                    // not the time the ETW event was emitted (which is in the header). Use it instead.
                    let timestamp_raw: u64 = parser.parse("EventTimeStamp");
                    let timestamp = timestamp_converter.convert_time(timestamp_raw);

                    if !context.threads.contains_key(&thread_id) { return }

                    // eprint!("{} {} {}", thread_id, e.EventHeader.TimeStamp, timestamp);

                    // TODO vlad -- the stacks are just in parser.buffer? From my reading the StackWalk event has
                    // EventTimeStamp: u64, StackProcess: u32, StackThread: u32, so 16 bytes shifted.

                    if is_arm64 {
                        // On ARM64, this is simpler -- stacks come in with full kernel and user frames.
                        // On x86_64 they seem to be split

                        // Iterate over the stack addresses, starting with the instruction pointer
                        let stack: Vec<StackFrame> = parser.buffer.chunks_exact(8) // iterate over 8 byte items
                            .map(|a| u64::from_ne_bytes(a.try_into().unwrap())) // parse into u64 (really we should just walk over u64 here, the world is LE)
                            .enumerate()
                            .map(|(i, addr)| {
                                if i == 0 {
                                    StackFrame::InstructionPointer(addr, context.stack_mode_for_address(addr))
                                } else {
                                    StackFrame::ReturnAddress(addr, context.stack_mode_for_address(addr))
                                }
                            })
                            .collect();

                        // TODO figure out how the on-cpu/off-cpu stuff works
                        context.add_sample(process_id, thread_id, timestamp, timestamp_raw, CpuDelta::ZERO, 1, stack);
                        return;
                    }

                    assert!(is_x86);

                    let mut stack: Vec<StackFrame> = Vec::with_capacity(parser.buffer.len() / 8);
                    let mut address_iter = parser.buffer.chunks_exact(8).map(|a| u64::from_ne_bytes(a.try_into().unwrap()));
                    let Some(first_frame_address) = address_iter.next() else { return };
                    let first_frame_stack_mode = context.stack_mode_for_address(first_frame_address);
                    stack.push(StackFrame::InstructionPointer(first_frame_address, first_frame_stack_mode));
                    for frame_address in address_iter {
                        let stack_mode = context.stack_mode_for_address(first_frame_address);
                        stack.push(StackFrame::ReturnAddress(frame_address, stack_mode));
                    }

                    if first_frame_stack_mode == StackMode::Kernel {
                        let mut thread = context.get_thread_mut(thread_id).unwrap();
                        if let Some(pending_stack ) = thread.pending_stacks.iter_mut().rev().find(|s| s.timestamp == timestamp_raw) {
                            if let Some(kernel_stack) = pending_stack.kernel_stack.as_mut() {
                                eprintln!("Multiple kernel stacks for timestamp {timestamp_raw} on thread {thread_id}");
                                kernel_stack.extend(&stack);
                            } else {
                                pending_stack.kernel_stack = Some(stack);
                            }
                        }
                        return;
                    }

                    // We now know that we have a user stack. User stacks always come last. Consume
                    // the pending stack with matching timestamp.

                    // the number of pending stacks at or before our timestamp
                    let num_pending_stacks = context.get_thread(thread_id).unwrap()
                        .pending_stacks.iter().take_while(|s| s.timestamp <= timestamp_raw).count();

                    let pending_stacks: VecDeque<_> = context.get_thread_mut(thread_id).unwrap().pending_stacks.drain(..num_pending_stacks).collect();

                    // Use this user stack for all pending stacks from this thread.
                    for pending_stack in pending_stacks {
                        let PendingStack {
                            timestamp,
                            kernel_stack,
                            off_cpu_sample_group,
                            on_cpu_sample_cpu_delta,
                        } = pending_stack;

                        if let Some(off_cpu_sample_group) = off_cpu_sample_group {
                            let OffCpuSampleGroup { begin_timestamp, end_timestamp, sample_count } = off_cpu_sample_group;

                            let cpu_delta_raw = {
                                let mut thread = context.get_thread_mut(thread_id).unwrap();
                                context_switch_handler.consume_cpu_delta(&mut thread.context_switch_data)
                            };
                            let cpu_delta = CpuDelta::from_nanos(cpu_delta_raw as u64 * timestamp_converter.raw_to_ns_factor);

                            // Add a sample at the beginning of the paused range.
                            // This "first sample" will carry any leftover accumulated running time ("cpu delta").
                            context.add_sample(process_id, thread_id, timestamp_converter.convert_time(begin_timestamp), begin_timestamp, cpu_delta, 1, stack.clone());

                            if sample_count > 1 {
                                // Emit a "rest sample" with a CPU delta of zero covering the rest of the paused range.
                                let weight = i32::try_from(sample_count - 1).unwrap_or(0) * 1;
                                context.add_sample(process_id, thread_id, timestamp_converter.convert_time(end_timestamp), end_timestamp, CpuDelta::ZERO, weight, stack.clone());
                            }
                        }

                        if let Some(cpu_delta) = on_cpu_sample_cpu_delta {
                            let timestamp_cvt = timestamp_converter.convert_time(timestamp);
                            if let Some(mut combined_stack) = kernel_stack {
                                combined_stack.extend_from_slice(&stack[..]);
                                context.add_sample(process_id, thread_id, timestamp_cvt, timestamp, cpu_delta, 1, combined_stack);
                            } else {
                                context.add_sample(process_id, thread_id, timestamp_cvt, timestamp, cpu_delta, 1, stack.clone());
                            }
                            stack_sample_count += 1;
                        }
                    }
                }
                "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                    let thread_id: u32 = parser.parse("ThreadId");

                    sample_count += 1;

                    if let Some(idle_handle) = context.get_idle_handle_if_appropriate(thread_id) {
                        let mut frames = Vec::new();
                        frames.push(FrameInfo {
                            frame: fxprof_processed_profile::Frame::Label(context.profile.borrow_mut().intern_string(if thread_id == 0 { "Idle" } else { "Other" })),
                            category_pair: user_category,
                            flags: FrameFlags::empty()
                        });
                        context.profile.borrow_mut().add_sample(idle_handle, timestamp, frames.into_iter(), Duration::ZERO.into(), 1);
                    }

                    let Some(mut thread) = context.get_thread_mut(thread_id) else { return };

                    let off_cpu_sample_group = context_switch_handler.handle_on_cpu_sample(timestamp_raw, &mut thread.context_switch_data);
                    let delta = context_switch_handler.consume_cpu_delta(&mut thread.context_switch_data);
                    let cpu_delta = CpuDelta::from_nanos(delta as u64 * timestamp_converter.raw_to_ns_factor);
                    thread.pending_stacks.push_back(PendingStack { timestamp: timestamp_raw, kernel_stack: None, off_cpu_sample_group, on_cpu_sample_cpu_delta: Some(cpu_delta) });
                }
                "MSNT_SystemTrace/PageFault/DemandZeroFault" => {
                    if !demand_zero_faults { return }

                    let thread_id: u32 = s.thread_id();
                    //println!("sample {}", thread_id);
                    sample_count += 1;

                    if let Some(idle_handle) = context.get_idle_handle_if_appropriate(thread_id) {
                        let mut frames = Vec::new();
                        frames.push(FrameInfo {
                            frame: fxprof_processed_profile::Frame::Label(context.profile.borrow_mut().intern_string(if thread_id == 0 { "Idle" } else { "Other" })),
                            category_pair: user_category,
                            flags: FrameFlags::empty()
                        });
                        context.profile.borrow_mut().add_sample(idle_handle, timestamp, frames.into_iter(), Duration::ZERO.into(), 1);
                    }

                    let Some(mut thread) = context.get_thread_mut(thread_id) else { return };

                    thread.pending_stacks.push_back(PendingStack { timestamp: timestamp_raw, kernel_stack: None, off_cpu_sample_group: None, on_cpu_sample_cpu_delta: Some(CpuDelta::from_millis(1.0)) });
                }
                "MSNT_SystemTrace/PageFault/VirtualAlloc" |
                "MSNT_SystemTrace/PageFault/VirtualFree" => {
                    if !context.is_interesting_process(e.EventHeader.ProcessId, None, None) || context.merge_threads {
                        return;
                    }

                    let thread_id = e.EventHeader.ThreadId;

                    let Some(memory_usage_counter) = context.get_or_create_memory_usage_counter(thread_id) else { return };
                    let Some(thread_handle) = context.get_thread(thread_id).map(|t| t.handle) else { return };

                    let region_size: u64 = parser.parse("RegionSize");

                    let is_free = s.name() == "MSNT_SystemTrace/PageFault/VirtualFree";
                    let delta_size = if is_free { -(region_size as f64) } else { region_size as f64 };
                    let op_name = if is_free { "VirtualFree" } else { "VirtualAlloc" };

                    //println!("{} VirtualFree({}) = {}", e.EventHeader.ProcessId, region_size, counter.value);

                    let text = event_properties_to_string(&s, &mut parser, None);
                    context.profile.borrow_mut().add_counter_sample(memory_usage_counter, timestamp, delta_size, 1);
                    context.profile.borrow_mut().add_marker(thread_handle, CategoryHandle::OTHER, op_name, SimpleMarker(text), MarkerTiming::Instant(timestamp));
                }
                "KernelTraceControl/ImageID/" => {
                    // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized during merge from
                    // from MSNT_SystemTrace/Image/Load but don't contain the full path of the binary, or the full debug info in one go.
                    // We go through "ImageID/" to capture pid/address + the original filename, then expect to see a "DbgID_RSDS" event
                    // with pdb and debug info. We link those up through the "libs" hash, and then finally add them to the process
                    // in Image/Load.

                    let process_id = s.process_id(); // there isn't a ProcessId field here
                    if !context.is_interesting_process(process_id, None, None) && process_id != 0 {
                        return
                    }

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let image_timestamp = parser.try_parse("TimeDateStamp").unwrap();
                    let image_size: u32 = parser.try_parse("ImageSize").unwrap();
                    let image_path: String = parser.try_parse("OriginalFileName").unwrap();
                    //eprintln!("ImageID pid: {} 0x{:x} {} {} {}", process_id, image_base, image_path, image_size, image_timestamp);
                    // "libs" is used as a cache to store the image path and size until we get the DbgID_RSDS event
                    if libs.contains_key(&(process_id, image_base)) {
                        // I see odd entries like this with no corresponding DbgID_RSDS:
                        //   ImageID pid: 3156 0xf70000 com.docker.cli.exe 49819648 0
                        // they all come from something docker-related. So don't panic on the duplicate.
                        //panic!("libs already contains key 0x{:x} for pid {}", image_base, process_id);
                    }
                    libs.insert((process_id, image_base), (image_path, image_size, image_timestamp));
                }
                "KernelTraceControl/ImageID/DbgID_RSDS" => {
                    let process_id = parser.try_parse("ProcessId").unwrap();
                    if !context.is_interesting_process(process_id, None, None) && process_id != 0 {
                        return
                    }

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let guid: GUID = parser.try_parse("GuidSig").unwrap();
                    let age: u32 = parser.try_parse("Age").unwrap();
                    let debug_id = DebugId::from_parts(Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4), age);
                    let pdb_path: String = parser.try_parse("PdbFileName").unwrap();
                    //let pdb_path = Path::new(&pdb_path);
                    let Some((ref path, image_size, timestamp)) = libs.remove(&(process_id, image_base)) else {
                        eprintln!("DbID_RSDS for image at 0x{:x} for pid {}, but has no entry in libs", image_base, process_id);
                        return
                    };
                    //eprintln!("DbgID_RSDS pid: {} 0x{:x} {} {} {} {}", process_id, image_base, path, debug_id, pdb_path, age);
                    let code_id = Some(format!("{timestamp:08X}{image_size:x}"));
                    let name = Path::new(path).file_name().unwrap().to_str().unwrap().to_owned();
                    let debug_name = Path::new(&pdb_path).file_name().unwrap().to_str().unwrap().to_owned();
                    let info = LibraryInfo { 
                        name,
                        debug_name,
                        path: path.clone(), 
                        code_id,
                        symbol_table: None, 
                        debug_path: pdb_path,
                        debug_id, 
                        arch: Some(context.arch.to_owned()),
                    };
                    if process_id == 0 || image_base >= context.kernel_min {
                        if let Some(oldinfo) = kernel_pending_libraries.get(&image_base) {
                            assert_eq!(*oldinfo, info);
                        } else {
                            kernel_pending_libraries.insert(image_base, info);
                        }
                    } else {
                        if let Some(mut process) = context.get_process_mut(process_id) {
                            process.pending_libraries.insert(image_base, info);
                        } else {
                            eprintln!("No process for pid {process_id}");
                        }
                    }

                }
                "MSNT_SystemTrace/Image/Load" | "MSNT_SystemTrace/Image/DCStart" => {
                    // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized from MSNT_SystemTrace/Image/Load
                    // but don't contain the full path of the binary. We go through a bit of a dance to store the information from those events
                    // in pending_libraries and deal with it here. We assume that the KernelTraceControl events come before the Image/Load event.

                    // the ProcessId field doesn't necessarily match s.process_id();
                    let process_id = parser.try_parse("ProcessId").unwrap();
                    if !context.is_interesting_process(process_id, None, None) && process_id != 0 {
                        return
                    }

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let image_size: u64 = parser.try_parse("ImageSize").unwrap();

                    let path: String = parser.try_parse("FileName").unwrap();
                    let path = context.map_device_path(&path);

                    let info = if process_id == 0 {
                        kernel_pending_libraries.remove(&image_base)
                    } else {
                        let mut process = context.get_process_mut(process_id).unwrap();
                        process.pending_libraries.remove(&image_base)
                    };
                    // If the file doesn't exist on disk we won't have KernelTraceControl/ImageID events
                    // This happens for the ghost drivers mentioned here: https://devblogs.microsoft.com/oldnewthing/20160913-00/?p=94305
                    if let Some(mut info) = info {
                        info.path = path;
                        let lib_handle = context.profile.borrow_mut().add_lib(info);
                        if process_id == 0 || image_base >= context.kernel_min {
                            context.profile.borrow_mut().add_kernel_lib_mapping(lib_handle, image_base, image_base + image_size as u64, 0);
                        } else {
                            let mut process = context.get_process_mut(process_id).unwrap();
                            process.regular_lib_mapping_ops.push(timestamp_raw, LibMappingOp::Add(LibMappingAdd {
                                start_avma: image_base,
                                end_avma: image_base + image_size as u64,
                                relative_address_at_start: 0,
                                info: LibMappingInfo::new_lib(lib_handle),
                            }));
                        }
                    }
                }
                "Microsoft-Windows-DxgKrnl/VSyncDPC/Info " => {
                    let timestamp = e.EventHeader.TimeStamp as u64;
                    let timestamp = timestamp_converter.convert_time(timestamp);

                    #[derive(Debug, Clone)]
                    pub struct VSyncMarker;

                    impl ProfilerMarker for VSyncMarker {
                        const MARKER_TYPE_NAME: &'static str = "Vsync";

                        fn json_marker_data(&self) -> Value {
                            json!({
                                "type": Self::MARKER_TYPE_NAME,
                                "name": ""
                            })
                        }

                        fn schema() -> MarkerSchema {
                            MarkerSchema {
                                type_name: Self::MARKER_TYPE_NAME,
                                locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable, MarkerLocation::TimelineOverview],
                                chart_label: Some("{marker.data.name}"),
                                tooltip_label: None,
                                table_label: Some("{marker.name} - {marker.data.name}"),
                                fields: vec![MarkerSchemaField::Dynamic(MarkerDynamicField {
                                    key: "name",
                                    label: "Details",
                                    format: MarkerFieldFormat::String,
                                    searchable: false,
                                })],
                            }
                        }
                    }

                    let gpu_thread = gpu_thread.get_or_insert_with(|| {
                        let gpu = context.profile.borrow_mut().add_process("GPU", 1, profile_start_instant);
                        context.profile.borrow_mut().add_thread(gpu, 1, profile_start_instant, false)
                    });
                    context.profile.borrow_mut().add_marker(*gpu_thread,
                        CategoryHandle::OTHER,
                        "Vsync",
                        VSyncMarker{},
                        MarkerTiming::Instant(timestamp)
                    );
                }
                "MSNT_SystemTrace/Thread/CSwitch" => {
                    let new_thread: u32 = parser.parse("NewThreadId");
                    let old_thread: u32 = parser.parse("OldThreadId");
                    let timestamp = e.EventHeader.TimeStamp as u64;
                    // println!("CSwitch {} -> {} @ {} on {}", old_thread, new_thread, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });

                    if let Some(mut old_thread) = context.get_thread_mut(old_thread) {
                        context_switch_handler.handle_switch_out(timestamp, &mut old_thread.context_switch_data);
                    }
                    if let Some(mut new_thread) = context.get_thread_mut(new_thread) {
                        let off_cpu_sample_group = context_switch_handler.handle_switch_in(timestamp, &mut new_thread.context_switch_data);
                        if let Some(off_cpu_sample_group) = off_cpu_sample_group {
                            new_thread.pending_stacks.push_back(PendingStack { timestamp, kernel_stack: None, off_cpu_sample_group: Some(off_cpu_sample_group), on_cpu_sample_cpu_delta: None });
                        }
                    }
                }
                "MSNT_SystemTrace/Thread/ReadyThread" => {
                    // these events can give us the unblocking stack
                    let _thread_id: u32 = parser.parse("TThreadId");
                }
                "V8.js/MethodLoad/" |
                "Microsoft-JScript/MethodRuntime/MethodDCStart" |
                "Microsoft-JScript/MethodRuntime/MethodLoad" => {
                    let method_name: String = parser.parse("MethodName");
                    let method_start_address: Address = parser.parse("MethodStartAddress");
                    let method_size: u64 = parser.parse("MethodSize");
                    // let source_id: u64 = parser.parse("SourceID");
                    let process_id = s.process_id();
                    let Some(process) = context.get_process(process_id) else { return; };

                    let process_jit_info = jscript_symbols.entry(s.process_id()).or_insert_with(|| {
                        let lib_handle = context.profile.borrow_mut().add_lib(LibraryInfo { name: format!("JIT-{process_id}"), debug_name: format!("JIT-{process_id}"), path: format!("JIT-{process_id}"), debug_path: format!("JIT-{process_id}"), debug_id: DebugId::nil(), code_id: None, arch: None, symbol_table: None });
                        ProcessJitInfo { lib_handle, jit_mapping_ops: LibMappingOpQueue::default(), next_relative_address: 0, symbols: Vec::new() }
                    });
                    let start_address = method_start_address.as_u64();
                    let relative_address = process_jit_info.next_relative_address;
                    process_jit_info.next_relative_address += method_size as u32;

                    let timestamp = e.EventHeader.TimeStamp as u64;
                    let timestamp = timestamp_converter.convert_time(timestamp);

                    if let Some(main_thread) = process.main_thread_handle {
                        context.profile.borrow_mut().add_marker(
                            main_thread,
                            CategoryHandle::OTHER,
                            "JitFunctionAdd",
                            JitFunctionAddMarker(method_name.to_owned()),
                            MarkerTiming::Instant(timestamp),
                        );
                    }
                    
                    let (category, js_frame) = jit_category_manager.classify_jit_symbol(&method_name, &mut *context.profile.borrow_mut());
                    let info = LibMappingInfo::new_jit_function(process_jit_info.lib_handle, category, js_frame);
                    process_jit_info.jit_mapping_ops.push(e.EventHeader.TimeStamp as u64, LibMappingOp::Add(LibMappingAdd {
                        start_avma: start_address,
                        end_avma: start_address + method_size,
                        relative_address_at_start: relative_address,
                        info
                    }));
                    process_jit_info.symbols.push(Symbol {
                        address: relative_address,
                        size: Some(method_size as u32),
                        name: method_name,
                    });
                }
                "V8.js/SourceLoad/" /*|
                "Microsoft-JScript/MethodRuntime/MethodDCStart" |
                "Microsoft-JScript/MethodRuntime/MethodLoad"*/ => {
                    let source_id: u64 = parser.parse("SourceID");
                    let url: String = parser.parse("Url");
                    //if s.process_id() == 6736 { dbg!(s.process_id(), &method_name, method_start_address, method_size); }
                    jscript_sources.insert(source_id, url);
                    //dbg!(s.process_id(), jscript_symbols.keys());

                }
                marker_name if marker_name.starts_with("Mozilla.FirefoxTraceLogger/") =>  {
                    let Some(marker_name) = marker_name.strip_prefix("Mozilla.FirefoxTraceLogger/").and_then(|s| s.strip_suffix("/")) else { return };

                    let thread_id = e.EventHeader.ThreadId;
                    let Some(thread) = context.get_thread(thread_id) else { return };

                    let text = event_properties_to_string(&s, &mut parser, Some(&["MarkerName", "StartTime", "EndTime", "Phase", "InnerWindowId", "CategoryPair"]));

                    /// From https://searchfox.org/mozilla-central/rev/0e7394a77cdbe1df5e04a1d4171d6da67b57fa17/mozglue/baseprofiler/public/BaseProfilerMarkersPrerequisites.h#355-360
                    const PHASE_INSTANT: u8 = 0;
                    const PHASE_INTERVAL: u8 = 1;
                    const PHASE_INTERVAL_START: u8 = 2;
                    const PHASE_INTERVAL_END: u8 = 3;

                    // We ignore e.EventHeader.TimeStamp and instead take the timestamp from the fields.
                    let start_time_qpc: u64 = parser.try_parse("StartTime").unwrap();
                    let end_time_qpc: u64 = parser.try_parse("EndTime").unwrap();
                    assert!(event_timestamps_are_qpc, "Inconsistent timestamp formats! ETW traces with Firefox events should be captured with QPC timestamps (-ClockType PerfCounter) so that ETW sample timestamps are compatible with the QPC timestamps in Firefox ETW trace events, so that the markers appear in the right place.");
                    let (phase, instant_time_qpc): (u8, u64) = match parser.try_parse("Phase") {
                        Ok(phase) => (phase, start_time_qpc),
                        Err(_) => {
                            // Before the landing of https://bugzilla.mozilla.org/show_bug.cgi?id=1882640 ,
                            // Firefox ETW trace events didn't have phase information, so we need to
                            // guess a phase based on the timestamps.
                            if start_time_qpc != 0 && end_time_qpc != 0 {
                                (PHASE_INTERVAL, 0)
                            } else if start_time_qpc != 0 {
                                (PHASE_INSTANT, start_time_qpc)
                            } else {
                                (PHASE_INSTANT, end_time_qpc)
                            }
                        }
                    };
                    let timing = match phase {
                        PHASE_INSTANT => MarkerTiming::Instant(timestamp_converter.convert_time(instant_time_qpc)),
                        PHASE_INTERVAL => MarkerTiming::Interval(timestamp_converter.convert_time(start_time_qpc), timestamp_converter.convert_time(end_time_qpc)),
                        PHASE_INTERVAL_START => MarkerTiming::IntervalStart(timestamp_converter.convert_time(start_time_qpc)),
                        PHASE_INTERVAL_END => MarkerTiming::IntervalEnd(timestamp_converter.convert_time(end_time_qpc)),
                        _ => panic!("Unexpected marker phase {phase}"),
                    };

                    if marker_name == "UserTiming" {
                        let name: String = parser.try_parse("name").unwrap();
                        context.profile.borrow_mut().add_marker(thread.handle, CategoryHandle::OTHER, "UserTiming", UserTimingMarker(name), timing);
                    } else if marker_name == "SimpleMarker" || marker_name == "Text" || marker_name == "tracing" {
                        let marker_name: String = parser.try_parse("MarkerName").unwrap();
                        context.profile.borrow_mut().add_marker(thread.handle, CategoryHandle::OTHER, &marker_name, SimpleMarker(text.clone()), timing);
                    } else {
                        context.profile.borrow_mut().add_marker(thread.handle, CategoryHandle::OTHER, marker_name, SimpleMarker(text.clone()), timing);
                    }
                }
                marker_name if marker_name.starts_with("Google.Chrome/") => {
                    let Some(marker_name) = marker_name.strip_prefix("Google.Chrome/").and_then(|s| s.strip_suffix("/")) else { return };
                    // a bitfield of keywords
                    bitflags! {
                        #[derive(PartialEq, Eq)]
                        pub struct KeywordNames: u64 {
                            const benchmark = 0x1;
                            const blink = 0x2;
                            const browser = 0x4;
                            const cc = 0x8;
                            const evdev = 0x10;
                            const gpu = 0x20;
                            const input = 0x40;
                            const netlog = 0x80;
                            const sequence_manager = 0x100;
                            const toplevel = 0x200;
                            const v8 = 0x400;
                            const disabled_by_default_cc_debug = 0x800;
                            const disabled_by_default_cc_debug_picture = 0x1000;
                            const disabled_by_default_toplevel_flow = 0x2000;
                            const startup = 0x4000;
                            const latency = 0x8000;
                            const blink_user_timing = 0x10000;
                            const media = 0x20000;
                            const loading = 0x40000;
                            const base = 0x80000;
                            const devtools_timeline = 0x100000;
                            const unused_bit_21 = 0x200000;
                            const unused_bit_22 = 0x400000;
                            const unused_bit_23 = 0x800000;
                            const unused_bit_24 = 0x1000000;
                            const unused_bit_25 = 0x2000000;
                            const unused_bit_26 = 0x4000000;
                            const unused_bit_27 = 0x8000000;
                            const unused_bit_28 = 0x10000000;
                            const unused_bit_29 = 0x20000000;
                            const unused_bit_30 = 0x40000000;
                            const unused_bit_31 = 0x80000000;
                            const unused_bit_32 = 0x100000000;
                            const unused_bit_33 = 0x200000000;
                            const unused_bit_34 = 0x400000000;
                            const unused_bit_35 = 0x800000000;
                            const unused_bit_36 = 0x1000000000;
                            const unused_bit_37 = 0x2000000000;
                            const unused_bit_38 = 0x4000000000;
                            const unused_bit_39 = 0x8000000000;
                            const unused_bit_40 = 0x10000000000;
                            const unused_bit_41 = 0x20000000000;
                            const navigation = 0x40000000000;
                            const ServiceWorker = 0x80000000000;
                            const edge_webview = 0x100000000000;
                            const diagnostic_event = 0x200000000000;
                            const __OTHER_EVENTS = 0x400000000000;
                            const __DISABLED_OTHER_EVENTS = 0x800000000000;
                        }
                    }

                    let thread_id = e.EventHeader.ThreadId;
                    let phase: String = parser.try_parse("Phase").unwrap();

                    let Some(thread) = context.get_thread(thread_id)  else { return };
                    let text = event_properties_to_string(&s, &mut parser, Some(&["Timestamp", "Phase", "Duration"]));

                    // We ignore e.EventHeader.TimeStamp and instead take the timestamp from the fields.
                    let timestamp_raw: u64 = parser.try_parse("Timestamp").unwrap();
                    let timestamp = timestamp_converter.convert_us(timestamp_raw);

                    let timing = match phase.as_str() {
                        "Begin" => MarkerTiming::IntervalStart(timestamp),
                        "End" => MarkerTiming::IntervalEnd(timestamp),
                        _ => MarkerTiming::Instant(timestamp),
                    };
                    let keyword = KeywordNames::from_bits(e.EventHeader.EventDescriptor.Keyword).unwrap();
                    if keyword == KeywordNames::blink_user_timing {
                        context.profile.borrow_mut().add_marker(thread.handle, CategoryHandle::OTHER, "UserTiming", UserTimingMarker(marker_name.to_owned()), timing);
                    } else {
                        context.profile.borrow_mut().add_marker(thread.handle, CategoryHandle::OTHER, marker_name, SimpleMarker(text.clone()), timing);
                    }
                }
                _ => {
                    let thread_id = e.EventHeader.ThreadId;
                    let Some(thread) = context.get_thread(thread_id) else { return };

                    let text = event_properties_to_string(&s, &mut parser, None);
                    let timing = MarkerTiming::Instant(timestamp);
                    let category = match categories.entry(s.provider_name()) {
                        Entry::Occupied(e) => *e.get(),
                        Entry::Vacant(e) => {
                            let category = context.profile.borrow_mut().add_category(e.key(), CategoryColor::Transparent);
                            *e.insert(category)
                        }
                    };

                    context.profile.borrow_mut().add_marker(thread.handle, category, s.name().split_once("/").unwrap().1, SimpleMarker(text), timing)
                    //println!("unhandled {}", s.name()) 
                }
            }
        }
    });

    if !result.is_ok() {
        dbg!(&result);
        std::process::exit(1);
    }

    let marker_spans = match marker_file {
        Some(marker_file) => get_markers(
            &Path::new(&marker_file),
            None, // extra_dir?
            timestamp_converter,
        )
        .expect("Could not get markers"),
        None => Vec::new(),
    };

    // Push queued samples into the profile.
    // We queue them so that we can get symbolicated JIT function names. To get symbolicated JIT function names,
    // we have to call profile.add_sample after we call profile.set_lib_symbol_table, and we don't have the
    // complete JIT symbol table before we've seen all JIT symbols.
    // (This is a rather weak justification. The better justification is that this is consistent with what
    // samply does on Linux and macOS, where the queued samples also want to respect JIT function names from
    // a /tmp/perf-1234.map file, and this file may not exist until the profiled process finishes.)
    let mut stack_frame_scratch_buf = Vec::new();
    for (process_id, process) in context.processes.iter() {
        let mut process = process.borrow_mut();
        ///let ProcessState { unresolved_samples, regular_lib_mapping_ops, main_thread_handle, .. } = process;
        let jitdump_lib_mapping_op_queues = match jscript_symbols.remove(&process_id) {
            Some(jit_info) => {
                context.profile.borrow_mut().set_lib_symbol_table(jit_info.lib_handle, Arc::new(SymbolTable::new(jit_info.symbols)));
                vec![jit_info.jit_mapping_ops]
            },
            None => Vec::new(),
        };
        // TODO proper threads, not main thread
        let marker_spans_on_thread = marker_spans.iter().map(|marker_span| {
            MarkerSpanOnThread {
                thread_handle: process.main_thread_handle.unwrap(),
                name: marker_span.name.clone(),
                start_time: marker_span.start_time,
                end_time: marker_span.end_time,
            }
        }).collect();

        let process_sample_data = ProcessSampleData::new(process.unresolved_samples.clone(),
                                                         process.regular_lib_mapping_ops.clone(),
                                                         jitdump_lib_mapping_op_queues,
                                                         None, marker_spans_on_thread);
                                                         //main_thread_handle.unwrap_or_else(|| panic!("process no main thread {:?}", process_id)));
        process_sample_data.flush_samples_to_profile(&mut *context.profile.borrow_mut(), user_category, kernel_category, &mut stack_frame_scratch_buf, &mut context.unresolved_stacks.borrow_mut(), &[])
    }

    /*if merge_threads {
        profile.add_thread(global_thread);
    } else {
        for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }
    }*/

    println!("Took {} seconds", (Instant::now()-processing_start_timestamp).as_secs_f32());
    println!("{} events, {} samples, {} dropped, {} stack-samples", event_count, sample_count, dropped_sample_count, stack_sample_count);
}
