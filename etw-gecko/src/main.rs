use std::{collections::{HashMap, HashSet, hash_map::Entry}, convert::TryInto, fs::File, io::{BufWriter}, path::{Path, PathBuf}, time::{Duration, Instant, SystemTime}};

use etw_reader::{GUID, open_trace, parser::{Parser, TryParse}, print_property, schema::SchemaLocator, write_property};
use serde_json::{Value, json, to_writer};

use fxprof_processed_profile::{Timestamp, MarkerDynamicField, MarkerFieldFormat, MarkerLocation, MarkerSchema, ReferenceTimestamp, MarkerSchemaField, MarkerTiming, ProfilerMarker, ThreadHandle, Profile, debugid::{self, CodeId}, SamplingInterval, CategoryPairHandle, ProcessHandle, LibraryInfo, CounterHandle};
use debugid::DebugId;
use uuid::Uuid;

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
                searchable: None,
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
    last_kernel_stack: Option<Vec<u64>>,
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

fn main() {
    let profile_start_instant = Timestamp::from_nanos_since_reference(0);
    let profile_start_system = SystemTime::now();

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut threads: HashMap<u32, ThreadState> = HashMap::new();
    let mut processes: HashMap<u32, ProcessHandle> = HashMap::new();
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

    let mut thread_index = 0;
    let mut sample_count = 0;
    let mut stack_sample_count = 0;
    let mut dropped_sample_count = 0;
    let mut timer_resolution: u32 = 0; // Resolution of the hardware timer, in units of 100 nanoseconds.
    let mut start_time: u64 = 0;
    let mut perf_freq: u64 = 0;
    let mut event_count = 0;
    let (mut global_thread, global_process) = if merge_threads {
        let global_process = profile.add_process("All processes", 1, profile_start_instant);
        (Some(profile.add_thread(global_process, 1, profile_start_instant, false)), Some(global_process))
    } else {
        (None, None)
    };
    let mut gpu_thread = None;

    let result = open_trace(Path::new(&trace_file), |e| {
        event_count += 1;
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            let _to_millis = |timestamp: i64| {
                (timestamp as f64 / perf_freq as f64) * 1000.
            };
            // XXX: be careful with this as it can overflow
            let to_nanos = |timestamp: u64| {
                timestamp * 1000 * 1000 * 1000 / perf_freq 
            };
            match s.name() {
                "MSNT_SystemTrace/EventTrace/Header" => {
                    let mut parser = Parser::create(&s);
                    timer_resolution = parser.parse("TimerResolution");
                    perf_freq = parser.parse("PerfFreq");

                    start_time = e.EventHeader.TimeStamp as u64;

                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        print_property(&mut parser, &property);
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
                            let process = processes[&dbg!(process_id)];
                            let handle = profile.add_thread(process, thread_id, thread_start_instant, false);
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
                            let handle = if let Some(global_thread) = global_thread {
                                global_thread
                            } else {
                                let process = processes[&process_id];
                                profile.add_thread(process, thread_id, thread_start_instant, false)
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
                        let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
                        let mut parser = Parser::create(&s);


                        let image_file_name: String = parser.parse("ImageFileName");
                        println!("process start {}", image_file_name);

                        let process_id: u32 = parser.parse("ProcessId");
                        if image_file_name.contains(process_target_name) {
                            println!("tracing {}", process_id);
                            process_targets.insert(process_id);
                            if global_process.is_none() {
                                processes.insert(process_id, profile.add_process(&image_file_name, process_id, timestamp));
                            }
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
                            let handle = if let Some(global_thread) = global_thread {
                                global_thread
                            } else {
                                profile.add_thread(processes[&process_id], thread_id, thread_start_instant, false)
                            };
                            let tb = e.insert(
                                ThreadState {
                                    handle,
                                    last_kernel_stack: None,
                                    last_kernel_stack_time: 0,
                                    last_sample_timestamp: None,
                                    merge_name: None,
                                    running_since_time: None,
                                    previous_sample_cpu_time: 0,
                                    total_running_time: 0,
                                    thread_id,
                                }
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
                    let mut stack = parser.buffer.chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .collect::<Vec<u64>>();
                    /*
                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        print_property(&mut parser, &property);
                    }*/
                    stack.reverse();

                    let mut add_sample = |thread: &mut ThreadState, timestamp, stack: Vec<u64>| {
                        let frames = stack.iter().map(|addr| (fxprof_processed_profile::Frame::ReturnAddress(*addr), user_category));
                        if let Some(global_thread) = global_thread {
                            let stack_frames = frames;
                            let mut frames = Vec::new();
                            let thread_name = thread.merge_name.as_ref().map(|x| strip_thread_numbers(x).to_owned()).unwrap_or_else(|| format!("thread {}", thread.thread_id));
                            frames.push((fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)), user_category));
                            frames.extend(stack_frames);
                            profile.add_sample(global_thread, timestamp, frames.into_iter(), Duration::ZERO.into(), 1);
                        } else {
                            let delta = thread.total_running_time - thread.previous_sample_cpu_time;
                            thread.previous_sample_cpu_time = thread.total_running_time;
                            let delta = Duration::from_nanos(to_nanos(delta as u64));
                            profile.add_sample(thread.handle, timestamp, frames, delta.into(), 1);
                        }
                    };

                    if is_kernel_address(stack[0], 8) {
                        //eprintln!("kernel ");
                        thread.last_kernel_stack_time = timestamp;
                        thread.last_kernel_stack = Some(stack);
                    } else {
                        if timestamp == thread.last_kernel_stack_time {
                            //eprintln!("matched");
                            if thread.last_kernel_stack.is_none() {
                                dbg!(thread.last_kernel_stack_time);
                            }
                            let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
                            stack.append(&mut thread.last_kernel_stack.take().unwrap());
                            add_sample(thread, timestamp, stack);
                        } else {
                            if let Some(kernel_stack) = thread.last_kernel_stack.take() {
                                // we're left with an unassociated kernel stack
                                dbg!(thread.last_kernel_stack_time);

                                let timestamp = Timestamp::from_nanos_since_reference(to_nanos(thread.last_kernel_stack_time - start_time));
                                add_sample(thread, timestamp, kernel_stack);
                            }
                            let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
                            add_sample(thread, timestamp, stack);
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
                                    let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));

                                    frames.push((fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)), user_category));
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
                                    let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));

                                    frames.push((fxprof_processed_profile::Frame::Label(profile.intern_string(&thread_name)), user_category));
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
                    let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
                    let thread_id = e.EventHeader.ThreadId;
                    let counter = match memory_usage.entry(e.EventHeader.ProcessId) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(entry) => {
                            entry.insert(MemoryUsage { counter: profile.add_counter("MemoryUsage", processes[&e.EventHeader.ProcessId]), value: 0. })
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
                        write_property(&mut text, &mut parser, &property);
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
                    let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
                    let thread_id = e.EventHeader.ThreadId;
                    let counter = match memory_usage.entry(e.EventHeader.ProcessId) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(entry) => {
                            entry.insert(MemoryUsage { counter: profile.add_counter("MemoryUsage", processes[&e.EventHeader.ProcessId]), value: 0. })
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
                        write_property(&mut text, &mut parser, &property);
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
                    let code_id = Some(CodeId::new(format!("{timestamp:08X}{image_size:x}")));
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
                        arch: Some("x86_64".into()), 
                        base_avma: image_base, 
                        avma_range: image_base..(image_base + image_size as u64) };
                    if process_id == 0 {
                        profile.add_kernel_lib(info)
                    } else {
                        let process = match global_process {
                            Some(global_process) => global_process,
                            None => processes[&dbg!(process_id)],
                        };
                        profile.add_lib(process, info)
                    }
                }
                "Microsoft-Windows-DxgKrnl/VSyncDPC/Info " => {
                    let timestamp = e.EventHeader.TimeStamp as u64;
                    let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));                
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
                                    searchable: None,
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
                _ => {
                    if s.name().starts_with("Google.Chrome/") {
                        let mut parser = Parser::create(&s);
                        let timestamp = e.EventHeader.TimeStamp as u64;
                        let timestamp = Timestamp::from_nanos_since_reference(to_nanos(timestamp - start_time));
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
                            write_property(&mut text, &mut parser, &property);
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

    if result.is_ok() {
    /*if merge_threads {
        profile.add_thread(global_thread);
    } else {
        for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }
    }*/

        let f = File::create("gecko.json").unwrap();
        to_writer(BufWriter::new(f), &profile).unwrap();
        println!("Took {} seconds", (Instant::now()-start).as_secs_f32());
        println!("{} events, {} samples, {} dropped, {} stack-samples", event_count, sample_count, dropped_sample_count, stack_sample_count);
    } else {
        dbg!(&result);
    }
}
