use std::{collections::{HashMap, hash_map::Entry}, fs::File, io::{BufReader, BufWriter}, path::Path, time::{Duration, Instant}};

use etw_log::{Guid, open_trace, parser::{Parser, TryParse}, schema::{Schema, SchemaLocator}, tdh::{self}};
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
}

fn main() {
    let mut profile = gecko_profile::ProfileBuilder::new(Instant::now(), "firefox", 34, Duration::new(40, 0));
    
    let mut schema_locator = SchemaLocator::new();
    let mut threads: HashMap<u32, ThreadState> = HashMap::new();
    let mut libs: HashMap<u64, (String, u32)> = HashMap::new();
    let process_target = 34596;
    let mut thread_index = 0;

    let mut log_file = open_trace(Path::new("D:\\Captures\\23-09-2021_17-21-32_thread-switch-bench.etl"), |e| {


            
        let mut process_event = |s: &Schema, mut parser: Parser| {
            let name = format!("{}/{}/{}", s.provider_name(), s.task_name(), s.opcode_name());

            match name.as_str() {
                "Kernel/Stack/StackWalk" => {
                    /*
                    let thread_id = properties["StackThread"].as_u64().unwrap() as u32;
                    let process_id = properties["StackProcess"].as_u64().unwrap() as u32;
                    if process_id != process_target {
                        continue;
                    }
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(), 
                        Entry::Vacant(e) => {
                            let tb = e.insert(
                                ThreadState {
                                    builder: ThreadBuilder::new(thread_id, thread_index, 0.0, false, false),
                                    last_kernel_stack: None,
                                    last_kernel_stack_time: 0,
                                }
                            );
                            thread_index += 1;
                            tb
                        }
                    };
                    let stack = properties["Stacks"].as_array().unwrap();
                    let mut stack: Vec<_> = stack.iter().rev().map(|x| x.as_u64().unwrap()).collect();
                    let timestamp = properties["EventTimeStamp"].as_u64().unwrap();
                    let to_milliseconds = 10000.;
                    if timestamp == 6037210290464 {
                        dbg!(&thread.last_kernel_stack);
                        dbg!(event);
                    }

                    if is_kernel_address(stack[0], 8) {
                        dbg!("kernel stack");
                        thread.last_kernel_stack_time = timestamp;
                        thread.last_kernel_stack = Some(stack);
                    } else {
                        if timestamp == thread.last_kernel_stack_time {
                            if thread.last_kernel_stack.is_none() {
                                dbg!(thread.last_kernel_stack_time);
                            }
                            stack.append(&mut thread.last_kernel_stack.take().unwrap());
                            thread.builder.add_sample(timestamp as f64 / to_milliseconds, &stack, 0);
                        } else if let Some(kernel_stack) = thread.last_kernel_stack.take() {
                            thread.builder.add_sample(thread.last_kernel_stack_time as f64 / to_milliseconds, &kernel_stack, 0);                        
                        }
                        //XXX: what unit are timestamps in the trace in?
                    }*/
                }
                "KernelTraceControl/ImageID/" => {
                    let process_id = s.process_id();
                    if process_id != process_target && process_id != 0 {
                        return;
                    }

                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    let image_size: u32 = parser.try_parse("ImageSize").unwrap();
                    let file_name = parser.try_parse("OriginalFileName").unwrap();
                    libs.insert(image_base, (file_name, image_size));
                }
                "KernelTraceControl/ImageID/DbgID_RSDS" => {
                    let process_id = s.process_id();
                    if process_id != process_target && process_id != 0 {
                        return;
                    }
                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();

                    let guid: Guid = parser.try_parse("GuidSig").unwrap();
                    let age: u32 = parser.try_parse("Age").unwrap();
                    let pdb_file_name: String = parser.try_parse("PdbFileName").unwrap();
                    if process_id == 0 && !pdb_file_name.contains("ntkrnlmp") {
                        return;
                    }
                    let (ref file_name, image_size) = libs[&image_base];
                    let uuid = uuid::Uuid::parse_str(&format!("{:?}", guid)).unwrap();
                    profile.add_lib(&pdb_file_name, &pdb_file_name, &uuid, age as u8, "x86_64", &(image_base..(image_base + image_size as u64)));
                }
                _ => {}
            }
            
            println!("{}", name);
        };
        let s = etw_log::schema_from_custom(e.clone());
        if let Some(s) = s {
    
            let mut parser = Parser::create(&s);
            process_event(&s, parser)
        } else {
            let s = tdh::schema_from_tdh(e.clone());  
            if let Ok(s) = s {
                let s = schema_locator.event_schema(e.clone()).unwrap();
    
                let mut parser = Parser::create(&s);
                process_event(&s, parser)
            } else {
                //eprintln!("unknown event {:x?}", e.EventHeader.ProviderId);
                
            }
        }
    });

    for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }

    let f = File::create("gecko.json").unwrap();
    to_writer(BufWriter::new(f), &profile.to_json()).unwrap();
    println!("Hello, world!");
}
