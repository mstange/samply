use std::{collections::{HashMap, HashSet, hash_map::Entry}, convert::TryInto, fs::File, io::BufWriter, path::Path, time::{Duration, Instant, SystemTime}, sync::Arc};

use etw_reader::{GUID, open_trace, parser::{Parser, TryParse, Address}, print_property, schema::SchemaLocator, write_property};
use lib_mappings::{LibMappingOpQueue, LibMappingOp, LibMappingAdd};
use serde_json::{Value, json, to_writer};
use fxprof_processed_profile::{Timestamp, MarkerDynamicField, MarkerFieldFormat, MarkerLocation, MarkerSchema, ReferenceTimestamp, MarkerSchemaField, MarkerTiming, ProfilerMarker, ThreadHandle, Profile, debugid, SamplingInterval, CategoryPairHandle, ProcessHandle, LibraryInfo, CounterHandle, FrameInfo, FrameFlags, LibraryHandle, CpuDelta, SymbolTable, Symbol};
use debugid::DebugId;

mod jit_category_manager;
mod lib_mappings;
mod process_sample_data;
mod stack_converter;
mod stack_depth_limiting_frame_iter;
mod timestamp_converter;
mod types;
mod unresolved_samples;

use jit_category_manager::JitCategoryManager;
use stack_converter::StackConverter;
use lib_mappings::LibMappingInfo;
use types::{StackFrame, StackMode};
use unresolved_samples::{UnresolvedSamples, UnresolvedStacks};
use uuid::Uuid;
use process_sample_data::ProcessSampleData;

use crate::timestamp_converter::TimestampConverter;


/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct TextMarker(pub String);

impl ProfilerMarker for TextMarker {
    const MARKER_TYPE_NAME: &'static str = "Text";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "name": self.0
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
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

fn is_kernel_address(ip: u64, pointer_size: u32) -> bool {
    if pointer_size == 4 {
        return ip >= 0x80000000;
    }
    return ip >= 0xFFFF000000000000;        // TODO I don't know what the true cutoff is.
}
struct ThreadState {
    // When merging threads `handle` is the global thread handle and we use `merge_name` to store the name
    handle: ThreadHandle,
    merge_name: Option<String>,
    last_kernel_stack: Option<Vec<StackFrame>>,
    last_kernel_stack_time: u64,
    last_sample_timestamp: Option<i64>,
    running_since_time: Option<i64>,
    total_running_time: i64,
    previous_sample_cpu_time: i64,
    thread_id: u32
}

impl ThreadState {
    fn new(handle: ThreadHandle, tid: u32) -> Self {
        ThreadState {
            handle,
            last_kernel_stack: None,
            last_kernel_stack_time: 0,
            last_sample_timestamp: None,
            merge_name: None,
            running_since_time: None,
            previous_sample_cpu_time: 0,
            total_running_time: 0,
            thread_id: tid
        }
    }
}


fn strip_thread_numbers(name: &str) -> &str {
    if let Some(hash) = name.find('#') {
        let (prefix, suffix) = name.split_at(hash);
        if suffix[1..].parse::<i32>().is_ok() {
            return prefix.trim();
        }
    }
    return name;
}

struct MemoryUsage {
    counter: CounterHandle,
    value: f64
}

struct ProcessJitInfo {
    lib_handle: LibraryHandle,
    jit_mapping_ops: LibMappingOpQueue,
    next_relative_address: u32,
    symbols: Vec<Symbol>,
}

struct ProcessState {
    process_handle: ProcessHandle,
    unresolved_samples: UnresolvedSamples,
    regular_lib_mapping_ops: LibMappingOpQueue,
    has_seen_first_thread: bool,
}

impl ProcessState {
    pub fn new(process_handle: ProcessHandle) -> Self {
        Self {
            process_handle,
            unresolved_samples: UnresolvedSamples::default(),
            regular_lib_mapping_ops: LibMappingOpQueue::default(),
            has_seen_first_thread: false,
        }
    }
}

fn main() {
    let profile_start_instant = Timestamp::from_nanos_since_reference(0);
    let profile_start_system = SystemTime::now();

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut threads: HashMap<u32, ThreadState> = HashMap::new();
    let mut processes: HashMap<u32, ProcessState> = HashMap::new();
    let mut memory_usage: HashMap<u32, MemoryUsage> = HashMap::new();

    let mut libs: HashMap<u64, (String, u32, u32)> = HashMap::new();
    let start = Instant::now();
    let mut pargs = pico_args::Arguments::from_env();
    let merge_threads = pargs.contains("--merge-threads");
    let include_idle = pargs.contains("--idle");
    let demand_zero_faults = pargs.contains("--demand-zero-faults");

    let trace_file: String = pargs.free_from_str().unwrap();

    let mut process_targets = HashSet::new();
    let mut process_target_name = None;
    if let Ok(process_filter) = pargs.free_from_str::<String>() {
        if let Ok(process_id) = process_filter.parse() {
            process_targets.insert(process_id);
        } else {
            println!("targeting {}", process_filter);
            process_target_name = Some(process_filter);
        }
    } else {
        println!("No process specified");
        std::process::exit(1);
    }
    
    let command_name = process_target_name.as_deref().unwrap_or("firefox");
    let mut profile = Profile::new(command_name, ReferenceTimestamp::from_system_time(profile_start_system),  SamplingInterval::from_nanos(122100)); // 8192Hz

    let user_category: CategoryPairHandle = profile.add_category("User", fxprof_processed_profile::CategoryColor::Yellow).into();
    let kernel_category: CategoryPairHandle = profile.add_category("Kernel", fxprof_processed_profile::CategoryColor::Orange).into();

    let mut jit_category_manager = JitCategoryManager::new();
    let mut unresolved_stacks = UnresolvedStacks::default();

    let mut thread_index = 0;
    let mut sample_count = 0;
    let mut stack_sample_count = 0;
    let mut dropped_sample_count = 0;
    let mut timer_resolution: u32 = 0; // Resolution of the hardware timer, in units of 100 nanoseconds.
    let mut event_count = 0;
    let (global_thread, global_process) = if merge_threads {
        let global_process = profile.add_process("All processes", 1, profile_start_instant);
        (Some(profile.add_thread(global_process, 1, profile_start_instant, true)), Some(global_process))
    } else {
        (None, None)
    };
    let mut gpu_thread = None;
    let mut jscript_symbols: HashMap<u32, ProcessJitInfo> = HashMap::new();
    let mut jscript_sources: HashMap<u64, String> = HashMap::new();

    // Make a dummy TimestampConverter. Once we've parsed the header, this will have correct values.
    let mut timestamp_converter = TimestampConverter {
        reference_raw: 0,
        raw_to_ns_factor: 1,
    };

    let result = open_trace(Path::new(&trace_file), |e| {
        event_count += 1;
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            match s.name() {
                "MSNT_SystemTrace/EventTrace/Header" => {
                    let mut parser = Parser::create(&s);
                    timer_resolution = parser.parse("TimerResolution");
                    let perf_freq: u64 = parser.parse("PerfFreq");
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
                    let mut parser = Parser::create(&s);
                    let interval: u32 = parser.parse("NewInterval");
                    let interval = SamplingInterval::from_nanos(interval as u64 * 100);
                    println!("Sample rate {}ms", interval.as_secs_f64() * 1000.);
                    profile.set_interval(interval);
                }
                "MSNT_SystemTrace/Thread/SetName" => {
                    let mut parser = Parser::create(&s);

                    let process_id: u32 = parser.parse("ProcessId");
                    if !process_targets.contains(&process_id) {
                        return;
                    }
                    let thread_id: u32 = parser.parse("ThreadId");
                    let thread_name: String = parser.parse("ThreadName");
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => {
                            let thread_start_instant = profile_start_instant;
                            let handle = match global_thread {
                                Some(global_thread) => global_thread,
                                None => {
                                    let process = processes[&process_id].process_handle;
                                    profile.add_thread(process, thread_id, thread_start_instant, false)
                                }
                            };
                            let tb = e.insert(
                                ThreadState::new(handle, thread_id)
                            );
                            thread_index += 1;
                            tb
                         }
                    };
                    if Some(thread.handle) != global_thread {
                        profile.set_thread_name(thread.handle, &thread_name);
                    }
                    thread.merge_name = Some(thread_name);
                }
                "MSNT_SystemTrace/Thread/Start" |
                "MSNT_SystemTrace/Thread/DCStart" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("TThreadId");
                    let process_id: u32 = parser.parse("ProcessId");
                    //assert_eq!(process_id,s.process_id());
                    //println!("thread_name pid: {} tid: {} name: {:?}", process_id, thread_id, thread_name);

                    if !process_targets.contains(&process_id) {
                        return;
                    }

                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => {
                            let thread_start_instant = profile_start_instant;
                            let handle = match global_thread {
                                Some(global_thread) => global_thread,
                                None => {
                                    let process = processes.get_mut(&process_id).unwrap();
                                    let is_main = !process.has_seen_first_thread;
                                    process.has_seen_first_thread = true;
                                    profile.add_thread(process.process_handle, thread_id, thread_start_instant, is_main)
                                }
                            };
                            let tb = e.insert(
                                ThreadState::new(handle, thread_id)
                            );
                            tb
                        }
                    };

                    let thread_name: Result<String, _> = parser.try_parse("ThreadName");
                    match thread_name {
                        Ok(thread_name) if !thread_name.is_empty() => {
                            if Some(thread.handle) != global_thread {
                                profile.set_thread_name(thread.handle, &thread_name);
                            }
                            thread.merge_name = Some(thread_name)
                        },
                        _ => {}
                    }
                }
                "MSNT_SystemTrace/Process/Start" |
                "MSNT_SystemTrace/Process/DCStart" => {
                    if let Some(process_target_name) = &process_target_name {
                        let timestamp = e.EventHeader.TimeStamp as u64;
                        let timestamp = timestamp_converter.convert_time(timestamp);
                        let mut parser = Parser::create(&s);


                        let image_file_name: String = parser.parse("ImageFileName");
                        println!("process start {}", image_file_name);

                        let process_id: u32 = parser.parse("ProcessId");
                        if image_file_name.contains(process_target_name) {
                            println!("tracing {}", process_id);
                            process_targets.insert(process_id);
                            let process_handle = match global_process {
                                Some(global_process) => global_process,
                                None => profile.add_process(&image_file_name, process_id, timestamp),
                            };
                            processes.insert(process_id, ProcessState::new(process_handle));
                        }
                    }
                }
                "MSNT_SystemTrace/StackWalk/Stack" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("StackThread");
                    let process_id: u32 = parser.parse("StackProcess");

                    if !process_targets.contains(&process_id) {
                        // eprintln!("not watching");
                        return;
                    }
                    
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => {
                            let thread_start_instant = profile_start_instant;
                            let handle = match global_thread {
                                Some(global_thread) => global_thread,
                                None => {
                                    let process = processes[&process_id].process_handle;
                                    profile.add_thread(process, thread_id, thread_start_instant, false)
                                }
                            };
                            let tb = e.insert(
                                ThreadState::new(handle, thread_id)
                            );
                            thread_index += 1;
                            tb
                        }
                    };
                    let timestamp: u64 = parser.parse("EventTimeStamp");
                   // eprint!("{} {} {}", thread_id, e.EventHeader.TimeStamp, timestamp);

                    // Only add callstacks if this stack is associated with a SampleProf event
                    if let Some(last) = thread.last_sample_timestamp {
                        if timestamp as i64 != last {
                            // eprintln!("doesn't match last");
                            return
                        }
                    } else {
                        // eprintln!("not last");
                        return
                    }
                    //eprintln!(" sample");

                    // read the stacks out manually
                    let mut stack: Vec<StackFrame> = parser.buffer.chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .map(|a| StackFrame::ReturnAddress(a, if is_kernel_address(a, 8) { StackMode::Kernel } else { StackMode::User }))
                    .collect();
                    /*
                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        print_property(&mut parser, &property);
                    }*/

                    let mut add_sample = |thread: &mut ThreadState, process: &mut ProcessState, timestamp: u64, cpu_delta: CpuDelta, stack: Vec<StackFrame>| {
                        let profile_timestamp = timestamp_converter.convert_time(timestamp);
                        let stack_index = unresolved_stacks.convert(stack.into_iter().rev());
                        let extra_label_frame = if let Some(global_thread) = global_thread {
                            let thread_name = thread.merge_name.as_ref().map(|x| strip_thread_numbers(x).to_owned()).unwrap_or_else(|| format!("thread {}", thread.thread_id));
                            Some(FrameInfo {
                                frame: fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)),
                                category_pair: user_category,
                                flags: FrameFlags::empty(),
                            })
                        } else { None };
                        process.unresolved_samples.add_sample(thread.handle, profile_timestamp, timestamp, stack_index, cpu_delta, 1, extra_label_frame);
                    };

                    if matches!(stack[0], StackFrame::ReturnAddress(_, StackMode::Kernel)) {
                        //eprintln!("kernel ");
                        thread.last_kernel_stack_time = timestamp;
                        thread.last_kernel_stack = Some(stack);
                    } else {
                        let delta = thread.total_running_time - thread.previous_sample_cpu_time;
                        thread.previous_sample_cpu_time = thread.total_running_time;
                        let cpu_delta = CpuDelta::from_nanos(delta as u64 * timestamp_converter.raw_to_ns_factor);
                        let process = processes.get_mut(&process_id).unwrap();
                        if timestamp == thread.last_kernel_stack_time {
                            //eprintln!("matched");
                            if thread.last_kernel_stack.is_none() {
                                dbg!(thread.last_kernel_stack_time);
                            }
                            // Prepend the kernel stack to the user stack, because `stack` is ordered from inside to outside
                            let mut user_stack = std::mem::replace(&mut stack, thread.last_kernel_stack.take().unwrap());
                            stack.append(&mut user_stack);
                            add_sample(thread, process, timestamp, cpu_delta, stack);
                        } else {
                            if let Some(kernel_stack) = thread.last_kernel_stack.take() {
                                // we're left with an unassociated kernel stack
                                dbg!(thread.last_kernel_stack_time);

                                add_sample(thread, process, thread.last_kernel_stack_time, CpuDelta::ZERO, kernel_stack);
                                // add_sample(thread, profile_timestamp, kernel_stack);
                            }
                            add_sample(thread, process, timestamp, cpu_delta, stack);
                            // process.unresolved_samples.add_sample(thread.handle, profile_timestamp, timestamp, stack_index, cpu_delta, 1);
                        }
                        stack_sample_count += 1;
                        //XXX: what unit are timestamps in the trace in?
                    }
                }
                "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("ThreadId");
                    //println!("sample {}", thread_id);
                    sample_count += 1;

                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(), 
                        Entry::Vacant(_) => {
                            if include_idle {
                                if let Some(global_thread) = global_thread {
                                    let mut frames = Vec::new();
                                    let thread_name = match thread_id {
                                        0 => "Idle",
                                        _ => "Other"
                                    };
                                    let timestamp = e.EventHeader.TimeStamp as u64;
                                    let timestamp = timestamp_converter.convert_time(timestamp);

                                    frames.push(FrameInfo {
                                        frame: fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)),
                                        category_pair: user_category,
                                        flags: FrameFlags::empty()
                                    });
                                    profile.add_sample(global_thread, timestamp, frames.into_iter(), Duration::ZERO.into(), 1);
                                }
                            }
                            dropped_sample_count += 1;
                            // We don't know what process this will before so just drop it for now
                            return;
                        }
                    };
                    // assert!(thread.running_since_time.is_some(), "thread {} not running @ {} on {}", thread_id, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                    thread.last_sample_timestamp = Some(e.EventHeader.TimeStamp);
                }
                "MSNT_SystemTrace/PageFault/DemandZeroFault" => {
                    if !demand_zero_faults { return }

                    let thread_id: u32 = s.thread_id();
                    //println!("sample {}", thread_id);
                    sample_count += 1;

                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(_) => {
                            if include_idle {
                                if let Some(global_thread) = global_thread {
                                    let mut frames = Vec::new();
                                    let thread_name = match thread_id {
                                        0 => "Idle",
                                        _ => "Other"
                                    };
                                    let timestamp = e.EventHeader.TimeStamp as u64;
                                    let timestamp = timestamp_converter.convert_time(timestamp);

                                    frames.push(FrameInfo {
                                        frame: fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)),
                                        category_pair: user_category,
                                        flags: FrameFlags::empty(),
                                    });

                                    profile.add_sample(global_thread, timestamp, frames.into_iter(), Duration::ZERO.into(), 1);
                                }
                            }
                            dropped_sample_count += 1;
                            // We don't know what process this will before so just drop it for now
                            return;
                        }
                    };
                    // assert!(thread.running_since_time.is_some(), "thread {} not running @ {} on {}", thread_id, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                    thread.last_sample_timestamp = Some(e.EventHeader.TimeStamp);
                }
                "MSNT_SystemTrace/PageFault/VirtualFree" => {
                    if !process_targets.contains(&e.EventHeader.ProcessId) {
                        return;
                    }
                    let mut parser = Parser::create(&s);
                    let timestamp = e.EventHeader.TimeStamp as u64;
                    let timestamp = timestamp_converter.convert_time(timestamp);
                    let thread_id = e.EventHeader.ThreadId;
                    let counter = match memory_usage.entry(e.EventHeader.ProcessId) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(entry) => {
                            entry.insert(MemoryUsage { counter: profile.add_counter(processes[&e.EventHeader.ProcessId].process_handle, "VirtualAlloc", "Memory", "Amount of VirtualAlloc allocated memory"), value: 0. })
                        }
                    };
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(_) => {
                            dropped_sample_count += 1;
                            // We don't know what process this will before so just drop it for now
                            return;
                        }
                    };
                    let timing =  MarkerTiming::Instant(timestamp);
                    let mut text = String::new();
                    let region_size: u64 = parser.parse("RegionSize");
                    counter.value -= region_size as f64;

                    println!("{} VirtualFree({}) = {}", e.EventHeader.ProcessId, region_size, counter.value);
                    
                    profile.add_counter_sample(counter.counter, timestamp, -(region_size as f64), 1);
                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        //dbg!(&property);
                        write_property(&mut text, &mut parser, &property, false);
                        text += ", "
                    }

                    //profile.add_marker(thread.handle, "VirtualFree", TextMarker(text), timing)
                }
                "MSNT_SystemTrace/PageFault/VirtualAlloc" => {
                    if !process_targets.contains(&e.EventHeader.ProcessId) {
                        return;
                    }
                    let mut parser = Parser::create(&s);
                    let timestamp = e.EventHeader.TimeStamp as u64;
                    let timestamp = timestamp_converter.convert_time(timestamp);
                    let thread_id = e.EventHeader.ThreadId;
                    let counter = match memory_usage.entry(e.EventHeader.ProcessId) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(entry) => {
                            entry.insert(MemoryUsage { counter: profile.add_counter(processes[&e.EventHeader.ProcessId].process_handle, "VirtualAlloc", "Memory", "Amount of VirtualAlloc allocated memory"), value: 0. })
                        }
                    };
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(_) => {
                            dropped_sample_count += 1;
                            // We don't know what process this will before so just drop it for now
                            return;
                        }
                    };
                    let timing =  MarkerTiming::Instant(timestamp);
                    let mut text = String::new();
                    let region_size: u64 = parser.parse("RegionSize");
                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        //dbg!(&property);
                        write_property(&mut text, &mut parser, &property, false);
                        text += ", "
                    }
                    counter.value += region_size as f64;
                    //println!("{}.{} VirtualAlloc({}) = {}",  e.EventHeader.ProcessId, thread_id, region_size, counter.value);
                    
                    profile.add_counter_sample(counter.counter, timestamp, (region_size as f64), 1);
                    profile.add_marker(thread.handle, "VirtualAlloc", TextMarker(text), timing)
                }
                "KernelTraceControl/ImageID/" => {

                    let process_id = s.process_id();
                    if !process_targets.contains(&process_id) && process_id != 0 {
                        return;
                    }
                    let mut parser = Parser::create(&s);

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let timestamp = parser.try_parse("TimeDateStamp").unwrap();
                    let image_size: u32 = parser.try_parse("ImageSize").unwrap();
                    let binary_path: String = parser.try_parse("OriginalFileName").unwrap();
                    let path = binary_path;
                    libs.insert(image_base, (path, image_size, timestamp));
                }
                "KernelTraceControl/ImageID/DbgID_RSDS" => {
                    let mut parser = Parser::create(&s);

                    let process_id = s.process_id();
                    if !process_targets.contains(&process_id) && process_id != 0 {
                        return;
                    }
                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();

                    let guid: GUID = parser.try_parse("GuidSig").unwrap();
                    let age: u32 = parser.try_parse("Age").unwrap();
                    let debug_id = DebugId::from_parts(Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4), age);
                    let pdb_path: String = parser.try_parse("PdbFileName").unwrap();
                    //let pdb_path = Path::new(&pdb_path);
                    let (ref path, image_size, timestamp) = libs[&image_base];
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
                        arch: Some("x86_64".into())
                    };
                    let lib_handle = profile.add_lib(info);
                    if process_id == 0 {
                        profile.add_kernel_lib_mapping(lib_handle, image_base, image_base + image_size as u64, 0);
                    } else {
                        let process = processes.get_mut(&process_id).unwrap();
                        process.regular_lib_mapping_ops.push(e.EventHeader.TimeStamp as u64, LibMappingOp::Add(LibMappingAdd {
                            start_avma: image_base,
                            end_avma: image_base + image_size as u64,
                            relative_address_at_start: 0,
                            info: LibMappingInfo::new_lib(lib_handle),
                        }));
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

                    let mut gpu_thread = gpu_thread.get_or_insert_with(|| {
                        let gpu = profile.add_process("GPU", 1, profile_start_instant);
                        profile.add_thread(gpu, 1, profile_start_instant, false)
                    });
                    profile.add_marker(*gpu_thread,
                        "Vsync",
                        VSyncMarker{},
                        MarkerTiming::Instant(timestamp)
                    );
                }
                "MSNT_SystemTrace/Thread/CSwitch" => {
                    let mut parser = Parser::create(&s);
                    let new_thread: u32 = parser.parse("NewThreadId");
                    let old_thread: u32 = parser.parse("OldThreadId");
                    // println!("CSwitch {} -> {} @ {} on {}", old_thread, new_thread, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                    if let Some(new_thread) = threads.get_mut(&new_thread) {
                        new_thread.running_since_time = Some(e.EventHeader.TimeStamp);
                    };
                    if let Some(old_thread) = threads.get_mut(&old_thread) {
                        if let Some(start_time) = old_thread.running_since_time {
                            old_thread.total_running_time += e.EventHeader.TimeStamp - start_time
                        }
                        old_thread.running_since_time = None;
                    };

                }
                "MSNT_SystemTrace/Thread/ReadyThread" => {
                    // these events can give us the unblocking stack
                    let mut parser = Parser::create(&s);
                    let _thread_id: u32 = parser.parse("TThreadId");
                }
                "V8.js/MethodLoad/" |
                "Microsoft-JScript/MethodRuntime/MethodDCStart" |
                "Microsoft-JScript/MethodRuntime/MethodLoad" => {
                    // these events can give us the unblocking stack
                    let mut parser = Parser::create(&s);
                    let method_name: String = parser.parse("MethodName");
                    let method_start_address: Address = parser.parse("MethodStartAddress");
                    let method_size: u64 = parser.parse("MethodSize");
                    let source_id: u64 = parser.parse("SourceID");
                    let process_id = s.process_id();
                    //if s.process_id() == 6736 { dbg!(s.process_id(), &method_name, method_start_address, method_size); }
                    let process_jit_info = jscript_symbols.entry(s.process_id()).or_insert_with(|| {
                        let lib_handle = profile.add_lib(LibraryInfo { name: format!("JIT-{process_id}"), debug_name: format!("JIT-{process_id}"), path: format!("JIT-{process_id}"), debug_path: format!("JIT-{process_id}"), debug_id: DebugId::nil(), code_id: None, arch: None, symbol_table: None });
                        ProcessJitInfo { lib_handle, jit_mapping_ops: LibMappingOpQueue::default(), next_relative_address: 0, symbols: Vec::new() }
                    });
                    let start_address = method_start_address.as_u64();
                    let relative_address = process_jit_info.next_relative_address;
                    process_jit_info.next_relative_address += method_size as u32;
                    
                    let (category, js_frame) = jit_category_manager.classify_jit_symbol(&method_name, &mut profile);
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
                    // these events can give us the unblocking stack
                    let mut parser = Parser::create(&s);
                    let source_id: u64 = parser.parse("SourceID");
                    let url: String = parser.parse("Url");
                    //if s.process_id() == 6736 { dbg!(s.process_id(), &method_name, method_start_address, method_size); }
                    jscript_sources.insert(source_id, url);
                    //dbg!(s.process_id(), jscript_symbols.keys());

                }
                _ => {
                    if s.name().starts_with("Google.Chrome/") {
                        let mut parser = Parser::create(&s);
                        let timestamp = e.EventHeader.TimeStamp as u64;
                        let timestamp = timestamp_converter.convert_time(timestamp);
                        let thread_id = e.EventHeader.ThreadId;
                        let phase: String = parser.try_parse("Phase").unwrap();
                        let thread = match threads.entry(thread_id) {
                            Entry::Occupied(e) => e.into_mut(), 
                            Entry::Vacant(_) => {
                                dropped_sample_count += 1;
                                // We don't know what process this will before so just drop it for now
                                return;
                            }
                        };
                        let timing = match phase.as_str() {
                            "Complete" => MarkerTiming::IntervalStart(timestamp),
                            "Complete End" => MarkerTiming::IntervalEnd(timestamp),
                            _ => MarkerTiming::Instant(timestamp),
                        };

                        let mut text = String::new();
                        for i in 0..s.property_count() {
                            let property = s.property(i);
                            //dbg!(&property);
                            write_property(&mut text, &mut parser, &property, false);
                            text += ", "
                        }

                        profile.add_marker(thread.handle, s.name().trim_start_matches("Google.Chrome/"), TextMarker(text), timing)
                    }
                     //println!("unhandled {}", s.name()) 
                    }
            }
            //println!("{}", name);
        }
    });

    if !result.is_ok() {
        dbg!(&result);
        std::process::exit(1);
    }

    // Push queued samples into the profile.
    // We queue them so that we can get symbolicated JIT function names. To get symbolicated JIT function names,
    // we have to call profile.add_sample after we call profile.set_lib_symbol_table, and we don't have the
    // complete JIT symbol table before we've seen all JIT symbols.
    // (This is a rather weak justification. The better justification is that this is consistent with what
    // samply does on Linux and macOS, where the queued samples also want to respect JIT function names from
    // a /tmp/perf-1234.map file, and this file may not exist until the profiled process finishes.)
    let mut stack_frame_scratch_buf = Vec::new();
    for (process_id, process) in processes {
        let ProcessState { unresolved_samples, regular_lib_mapping_ops, .. } = process;
        let jitdump_lib_mapping_op_queues = match jscript_symbols.remove(&process_id) {
            Some(jit_info) => {
                profile.set_lib_symbol_table(jit_info.lib_handle, Arc::new(SymbolTable::new(jit_info.symbols)));
                vec![jit_info.jit_mapping_ops]
            },
            None => Vec::new(),
        };
        let process_sample_data = ProcessSampleData::new(unresolved_samples, regular_lib_mapping_ops, jitdump_lib_mapping_op_queues, None);
        process_sample_data.flush_samples_to_profile(&mut profile, user_category, kernel_category, &mut stack_frame_scratch_buf, &mut unresolved_stacks, &[])
    }

    /*if merge_threads {
        profile.add_thread(global_thread);
    } else {
        for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }
    }*/

    let f = File::create("gecko.json").unwrap();
    to_writer(BufWriter::new(f), &profile).unwrap();
    println!("Took {} seconds", (Instant::now()-start).as_secs_f32());
    println!("{} events, {} samples, {} dropped, {} stack-samples", event_count, sample_count, dropped_sample_count, stack_sample_count);
}
