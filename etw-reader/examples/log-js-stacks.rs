use std::collections::hash_map::Entry;
use std::collections::Bound::{Included, Unbounded};
use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;
use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Address, Parser, TryParse};
use etw_reader::schema::SchemaLocator;

/// A single symbol from a [`SymbolTable`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol {
    /// The symbol's address, as a "relative address", i.e. relative to the library's base address.
    pub address: u32,
    /// The symbol's size, if known. This is often just set based on the address of the next symbol.
    pub size: Option<u32>,
    /// The symbol name.
    pub name: String,
}

struct Event {
    name: String,
    timestamp: i64,
    stack: Option<Vec<u64>>,
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
    let mut jscript_symbols: HashMap<u32, BTreeMap<u64, (u64, String)>> = HashMap::new();
    let jscript_sources: HashMap<u64, String> = HashMap::new();
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

                    println!("Sample");
                    let thread = match threads.entry(thread_id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => e.insert(ThreadState::new(process_id)),
                    };
                    let timestamp: u64 = parser.parse("EventTimeStamp");

                    let stack: Vec<u64> = parser
                        .buffer
                        .chunks_exact(8)
                        .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                        .collect();

                    let mut js_stack = Vec::new();
                    let mut child = String::new();
                    let mut child_addr = 0;

                    stack.iter().for_each(|addr| {
                        if let Some(syms) = jscript_symbols.get(&process_id) {
                            if let Some(sym) = syms.range((Unbounded, Included(addr))).last() {
                                if *addr < *sym.0 + sym.1 .0 {
                                    js_stack.push((sym.1 .1.clone(), *addr));
                                    //println!("found match for {} calls {:x}:{}", sym.1.1, child_addr, child);
                                    child.clone_from(&sym.1 .1);
                                    child_addr = *sym.0;
                                }
                            }
                        }
                        //println!("{:x}", addr);
                    });

                    for i in 0..js_stack.len() {
                        if js_stack[i].0.contains("runSync")
                            && i > 2
                            && js_stack[i - 2].0.contains("click")
                        {
                            println!(
                                "found match {} {:x}/{} {}",
                                js_stack[i].0,
                                js_stack[i - 1].1,
                                js_stack[i - 1].0,
                                js_stack[i - 2].0
                            );
                        }
                    }
                }
                "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                    let mut parser = Parser::create(&s);

                    thread_id = parser.parse("ThreadId");
                }
                "V8.js/MethodLoad/"
                | "Microsoft-JScript/MethodRuntime/MethodDCStart"
                | "Microsoft-JScript/MethodRuntime/MethodLoad" => {
                    let mut parser = Parser::create(&s);
                    let method_name: String = parser.parse("MethodName");
                    let method_start_address: Address = parser.parse("MethodStartAddress");
                    let method_size: u64 = parser.parse("MethodSize");
                    if method_start_address.as_u64() <= 0x7ffde0297cc0
                        && method_start_address.as_u64() + method_size >= 0x7ffde0297cc0
                    {
                        println!(
                            "before: {} {:x} {}",
                            method_name,
                            method_start_address.as_u64(),
                            method_size
                        );
                    }

                    let source_id: u64 = parser.parse("SourceID");
                    let process_id = s.process_id();
                    if method_name.contains("getNearestLContainer")
                        || method_name.contains("277:53")
                    {
                        println!(
                            "load {} {} {:x}",
                            method_name,
                            method_size,
                            method_start_address.as_u64()
                        );
                    }
                    let syms = jscript_symbols.entry(s.process_id()).or_default();
                    //let name_and_file = format!("{} {}", method_name, jscript_sources.get(&source_id).map(|x| x.as_ref()).unwrap_or("?"));
                    let start_address = method_start_address.as_u64();
                    let mut overlaps = Vec::new();
                    for sym in syms.range_mut((
                        Included(start_address),
                        Included(start_address + method_size),
                    )) {
                        if method_name != sym.1 .1
                            || start_address != *sym.0
                            || method_size != sym.1 .0
                        {
                            println!(
                                "overlap {} {} {} -  {:?}",
                                method_name, start_address, method_size, sym
                            );
                            overlaps.push(*sym.0);
                        } else {
                            println!(
                                "overlap same {} {} {} -  {:?}",
                                method_name, start_address, method_size, sym
                            );
                        }
                    }
                    for sym in overlaps {
                        syms.remove(&sym);
                    }

                    syms.insert(start_address, (method_size, method_name));
                    //dbg!(s.process_id(), jscript_symbols.keys());
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
        } else if pattern.is_none() {
            /*println!(
                "unknown event {:x?}:{}",
                e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode
            );*/
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
        if !state.unfinished_kernel_stacks.is_empty() {
            println!(
                "thread `{tid}` of {} has {} unfinished kernel stacks",
                state.process_id,
                state.unfinished_kernel_stacks.len()
            );
            for stack in state.unfinished_kernel_stacks {
                println!("   {}", events[stack].timestamp);
            }
        }
    }
}
