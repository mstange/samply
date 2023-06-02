use etw_reader::{
    open_trace,
    parser::{Parser, TryParse},
    print_property,
    schema::SchemaLocator,
};
use std::{
    cell::Cell,
    collections::{hash_map::Entry, HashMap},
    convert::TryInto,
    path::Path,
};
use windows::Win32::System::Diagnostics::Etw;

fn is_kernel_address(ip: u64, pointer_size: u32) -> bool {
    if pointer_size == 4 {
        return ip >= 0x80000000;
    }
    return ip >= 0xFFFF000000000000; // TODO I don't know what the true cutoff is.
}

struct Event {
    name: String,
    timestamp: i64,
    thread_id: u32,
    stack: Option<Vec<u64>>,
    cpu: u16,
}

struct ThreadState {
    process_id: u32,
    unfinished_kernel_stacks: Vec<usize>,
}
impl ThreadState {
    fn new(process_id: u32) -> Self {
        ThreadState {
            process_id,
            unfinished_kernel_stacks: Vec::new(),
        }
    }
}

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let pattern = std::env::args().nth(2);
    let mut processes = HashMap::new();
    let mut events: Vec<Event> = Vec::new();
    let mut threads = HashMap::new();
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        //dbg!(e.EventHeader.TimeStamp);

        let s = schema_locator.event_schema(e);
        let mut thread_id = e.EventHeader.ThreadId;
        if let Ok(s) = s {
            match s.name() {
                "MSNT_SystemTrace/StackWalk/Stack" => {
                    let mut parser = Parser::create(&s);

                    let thread_id: u32 = parser.parse("StackThread");
                    let process_id: u32 = parser.parse("StackProcess");


                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => e.insert(ThreadState::new(process_id)),
                    };
                    let timestamp: u64 = parser.parse("EventTimeStamp");

                    let mut stack: Vec<u64> = parser
                        .buffer
                        .chunks_exact(8)
                        .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                        .collect();

                    let ends_in_kernel = is_kernel_address(*stack.last().unwrap(), 8);
                    let mut i = events.len() - 1;
                    let mut found_event: Option<usize> = None;
                    let cpu = unsafe { e.BufferContext.Anonymous.ProcessorIndex };
                    while i > 0 {
                        if events[i].timestamp < timestamp as i64 {
                            break;
                        }
                        // sometimes the thread_id won't match (virtualalloc?)
                        // we adjust the thread_id of the SampleProf event to get this to work
                        // otherwise TraceLog will use the cpu index
                        if events[i].timestamp == timestamp as i64
                            && events[i].thread_id == thread_id
                        {
                            if let Some(first_event) = found_event {
                                println!(
                                "more than one associated event {}/{}:{}@{} {}/{}:{}@{} {}/{}@{}",
                                first_event,
                                events[first_event].name,
                                events[first_event].thread_id,
                                events[first_event].cpu,
                                i,
                                events[i].name,
                                events[i].thread_id,
                                events[i].cpu,
                                timestamp,
                                thread_id,
                                cpu,
                            );
                            }
                            if ends_in_kernel {
                                match &mut events[i].stack {
                                    Some(existing_stack) => {
                                        // Sometimes the kernel will call back into userspace (KeUserModeCallback)
                                        // this can cause there to be multiple stacks that end in a kernel address.
                                        //
                                        // Microsoft's TraceLog library seems to discard the initial kernel stack replacing
                                        // it with a subsequent one which seems wrong because the initial stack contains
                                        // the address which matches the 'InstructionPointer' field in the SampleProf event.
                                        // 
                                        // Instead of discarding, we concatenate the stacks
                                        assert!(thread.unfinished_kernel_stacks.contains(&i));
                                        existing_stack.extend_from_slice(&stack[..])
                                    }
                                    None => {
                                        thread.unfinished_kernel_stacks.push(i);
                                        events[i].stack = Some(stack.clone());
                                    }
                                };
                            } else {
                                for e in &thread.unfinished_kernel_stacks {
                                    events[*e]
                                        .stack
                                        .as_mut()
                                        .unwrap()
                                        .extend_from_slice(&stack[..]);
                                }
                                match &mut events[i].stack {
                                    Some(_) => {
                                        // any existing stacks should only have come from kernel stacks
                                        assert!(thread.unfinished_kernel_stacks.contains(&i));
                                    }
                                    None => {
                                        events[i].stack = Some(stack.clone());
                                    }
                                };
                                thread.unfinished_kernel_stacks.clear();

                            }

                            found_event = Some(i);
                        }
                        i -= 1;
                    }

                    if found_event.is_none() {
                        println!("no matching event");
                    }
                }
                "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                    let mut parser = Parser::create(&s);

                    thread_id = parser.parse("ThreadId");
                }
                _ => {}
            }
            if let "MSNT_SystemTrace/Process/Start"
            | "MSNT_SystemTrace/Process/DCStart"
            | "MSNT_SystemTrace/Process/DCEnd" = s.name()
            {
                let mut parser = Parser::create(&s);

                let image_file_name: String = parser.parse("ImageFileName");
                let process_id: u32 = parser.parse("ProcessId");
                processes.insert(process_id, image_file_name);
            }

            events.push(Event {
                name: s.name().to_owned(),
                timestamp: e.EventHeader.TimeStamp,
                thread_id,
                cpu: unsafe { e.BufferContext.Anonymous.ProcessorIndex },
                stack: None,
            });
        } else {
            if pattern.is_none() {
                /*println!(
                    "unknown event {:x?}:{}",
                    e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode
                );*/
            }
        }
    })
    .unwrap();
    for e in &mut events {
        if let Some(stack) = &e.stack {
            println!("{} {}", e.timestamp, e.name);
            for addr in stack {
                println!("    {:x}", addr);
            }
        }
    }
    for (tid, state) in threads {
        if state.unfinished_kernel_stacks.len() > 0 {
            println!("thread `{tid}` of {} has {} unfinished kernel stacks", state.process_id, state.unfinished_kernel_stacks.len());
            for stack in state.unfinished_kernel_stacks {
                println!("   {}", events[stack].timestamp);
            }
        }
    }
}
