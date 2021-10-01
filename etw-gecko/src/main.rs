use std::{collections::{HashMap, hash_map::Entry}, convert::TryInto, fs::File, io::{BufReader, BufWriter}, path::Path, time::{Duration, Instant}};

use etw_reader::{Guid, etw_types::EventPropertyInfo, open_trace, parser::{Parser, TryParse}, schema::{Schema, SchemaLocator}, tdh::{self}, tdh_types::{Property, TdhInType}};
use serde_json::to_writer;

use crate::gecko_profile::ThreadBuilder;

mod gecko_profile;

fn is_kernel_address(ip: u64, pointer_size: u32) -> bool {
    if pointer_size == 4 {
        return ip >= 0x80000000;
    }
    return ip >= 0xFFFF000000000000;        // TODO I don't know what the true cutoff is.
}
struct ThreadState {
    builder: ThreadBuilder,
    last_kernel_stack: Option<Vec<u64>>,
    last_kernel_stack_time: u64,
    last_sample_timestamp: Option<i64>
}

fn print_property(parser: &mut Parser, property: &Property) {
    print!("{} = ", property.name);
    match property.in_type() {
        TdhInType::InTypeUnicodeString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeAnsiString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt32 => println!("{:?}", TryParse::<u32>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt8 => println!("{:?}", TryParse::<u8>::try_parse(parser, &property.name)),
        TdhInType::InTypePointer => println!("{:?}", TryParse::<u64>::try_parse(parser, &property.name)),
        TdhInType::InTypeInt64 => println!("{:?}", TryParse::<i64>::try_parse(parser, &property.name)),
        TdhInType::InTypeGuid => println!("{:?}", TryParse::<Guid>::try_parse(parser, &property.name)),
        _ => println!("Unknown {:?}", property.in_type())
    }
}

fn main() {
    let mut profile = gecko_profile::ProfileBuilder::new(Instant::now(), "firefox", 34, Duration::new(40, 0));
    
    let mut schema_locator = SchemaLocator::new();
    let mut threads: HashMap<u32, ThreadState> = HashMap::new();
    let mut libs: HashMap<u64, (String, u32)> = HashMap::new();
    let start = Instant::now();
    let process_target = 33808;
    let process_target = 26956;

    let mut thread_index = 0;

    //let mut log_file = open_trace(Path::new("D:\\Captures\\30-09-2021_09-26-46_firefox.etl"), |e| {
    let mut log_file = open_trace(Path::new("D:\\Captures\\30-09-2021_15-37-50_firefox.etl"), |e| {

        let mut process_event = |s: &Schema| {
            let name = format!("{}/{}/{}", s.provider_name(), s.task_name(), s.opcode_name());
            match name.as_str() {
                "MSNT_SystemTrace/Thread/DCStart" => {
                    let process_id = s.process_id();
                    if process_id != process_target {
                        return;
                    }
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("TThreadId");
                    let thread_name: String = parser.parse("ThreadName");
                    println!("thread_name {}", &thread_name);

                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(), 
                        Entry::Vacant(e) => {
                            let tb = e.insert(
                                ThreadState {
                                    builder: ThreadBuilder::new(process_id, thread_index, 0.0, false, false),
                                    last_kernel_stack: None,
                                    last_kernel_stack_time: 0,
                                    last_sample_timestamp: None
                                }
                            );
                            thread_index += 1;
                            tb
                        }
                    };
                    if !thread_name.is_empty() {
                        thread.builder.set_name(&thread_name);
                    }

                }
                "MSNT_SystemTrace/StackWalk/Stack" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("StackThread");
                    let process_id: u32 = parser.parse("StackProcess");
                    if process_id != process_target {
                        return;
                    }
                    
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(), 
                        Entry::Vacant(e) => {
                            let tb = e.insert(
                                ThreadState {
                                    builder: ThreadBuilder::new(process_id, thread_index, 0.0, false, false),
                                    last_kernel_stack: None,
                                    last_kernel_stack_time: 0,
                                    last_sample_timestamp: None
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
                            //eprintln!("");
                            return
                        }
                    } else {
                        //eprintln!("");
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
                    let to_milliseconds = 10000.;

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
                            stack.append(&mut thread.last_kernel_stack.take().unwrap());
                            thread.builder.add_sample(timestamp as f64 / to_milliseconds, &stack, 0);
                        } else if let Some(kernel_stack) = thread.last_kernel_stack.take() {
                            // we're left with an unassociated kernel stack
                            dbg!(thread.last_kernel_stack_time);
                            thread.builder.add_sample(thread.last_kernel_stack_time as f64 / to_milliseconds, &kernel_stack, 0);                        
                        }
                        //XXX: what unit are timestamps in the trace in?
                    }
                }
                "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("ThreadId");

                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(), 
                        Entry::Vacant(e) => {
                            let tb = e.insert(
                                ThreadState {
                                    builder: ThreadBuilder::new(process_target, thread_index, 0.0, false, false),
                                    last_kernel_stack: None,
                                    last_kernel_stack_time: 0,
                                    last_sample_timestamp: None
                                }
                            );
                            thread_index += 1;
                            tb
                        }
                    };

                    thread.last_sample_timestamp = Some(e.EventHeader.TimeStamp);
                }
                "KernelTraceControl/ImageID/" => {

                    let process_id = s.process_id();
                    if process_id != process_target && process_id != 0 {
                        return;
                    }
                    let mut parser = Parser::create(&s);

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let image_size: u32 = parser.try_parse("ImageSize").unwrap();
                    let file_name = parser.try_parse("OriginalFileName").unwrap();
                    libs.insert(image_base, (file_name, image_size));
                }
                "KernelTraceControl/ImageID/DbgID_RSDS" => {
                    let mut parser = Parser::create(&s);

                    let process_id = s.process_id();
                    if process_id != process_target && process_id != 0 {
                        return;
                    }
                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();

                    let guid: Guid = parser.try_parse("GuidSig").unwrap();
                    let age: u32 = parser.try_parse("Age").unwrap();
                    let pdb_file_name: String = parser.try_parse("PdbFileName").unwrap();
                    // we only allow some kernel libraries so that we don't have to download symbols for all the modules that have been loaded
                    if process_id == 0 && !(pdb_file_name.contains("ntkrnlmp") || pdb_file_name.contains("win32k")) {
                        return;
                    }
                    let (ref file_name, image_size) = libs[&image_base];
                    let uuid = uuid::Uuid::parse_str(&format!("{:?}", guid)).unwrap();
                    profile.add_lib(&pdb_file_name, &pdb_file_name, &uuid, age as u8, "x86_64", &(image_base..(image_base + image_size as u64)));
                }
                _ => {}
            }
            
            //println!("{}", name);
        };
        let s = etw_reader::schema_from_custom(e.clone());
        if let Some(s) = s {
            process_event(&s)
        } else {
            let s = tdh::schema_from_tdh(e.clone());  
            if let Ok(s) = s {
                let s = schema_locator.event_schema(e.clone()).unwrap();
                    process_event(&s)
            } else {
                //eprintln!("unknown event {:x?}", e.EventHeader.ProviderId);
                
            }
        }
    });

    for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }

    let f = File::create("gecko.json").unwrap();
    to_writer(BufWriter::new(f), &profile.to_json()).unwrap();
    println!("Took {} seconds", (Instant::now()-start).as_secs_f32())
}
