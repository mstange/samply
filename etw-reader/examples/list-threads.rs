use std::collections::HashMap;
use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;

struct Process {
    image_file_name: String,
}

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut processes = HashMap::new();
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            // DCEnd is used '-Buffering' traces
            match s.name() {
                "MSNT_SystemTrace/Process/Start"
                | "MSNT_SystemTrace/Process/DCStart"
                | "MSNT_SystemTrace/Process/DCEnd" => {
                    let mut parser = Parser::create(&s);

                    let image_file_name: String = parser.parse("ImageFileName");
                    let process_id: u32 = parser.parse("ProcessId");
                    let command_line: String = parser.parse("CommandLine");
                    println!("{} {} {}", image_file_name, process_id, command_line);

                    processes.insert(process_id, Process { image_file_name });
                }
                "MSNT_SystemTrace/Thread/Start" | "MSNT_SystemTrace/Thread/DCStart" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("TThreadId");
                    let process_id: u32 = parser.parse("ProcessId");
                    println!(
                        "thread process {}({}) tid {}",
                        processes[&process_id].image_file_name, process_id, thread_id
                    );
                    //assert_eq!(process_id,s.process_id());

                    let thread_name: Result<String, _> = parser.try_parse("ThreadName");
                    match thread_name {
                        Ok(thread_name) if !thread_name.is_empty() => {
                            println!(
                                "thread_name pid: {} tid: {} name: {:?}",
                                process_id, thread_id, thread_name
                            );
                        }
                        _ => {}
                    }
                }
                "MSNT_SystemTrace/Thread/SetName" => {
                    let mut parser = Parser::create(&s);

                    let process_id: u32 = parser.parse("ProcessId");
                    let thread_id: u32 = parser.parse("ThreadId");
                    let thread_name: String = parser.parse("ThreadName");
                    /*
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
                    };*/
                }
                _ => {}
            }
        }
    })
    .expect("failed");
}
