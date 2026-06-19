// Dump the leaf-first kernel/user pattern of the first N StackWalk fragments,
// interleaved with the SampleProf events, so we can see how fragments are
// ordered and split.
//
// Usage: cargo run --release --example stack-dump -- <path-to-etl> [N]

use std::convert::TryInto;
use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;

fn is_kernel(ip: u64) -> bool {
    ip >= 0xFFFF000000000000
}

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);

    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let mut printed = 0usize;

    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        if printed >= limit {
            return;
        }
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };
        match s.name() {
            "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("ThreadId");
                let ip: u64 = parser.try_parse("InstructionPointer").unwrap_or(0);
                println!(
                    "SAMPLE  ts={:>16} tid={:<6} ip={:016x} {}",
                    e.EventHeader.TimeStamp,
                    tid,
                    ip,
                    if is_kernel(ip) { "K" } else { "U" }
                );
                printed += 1;
            }
            "MSNT_SystemTrace/StackWalk/Stack" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("StackThread");
                let ts: u64 = parser.parse("EventTimeStamp");
                let stack: Vec<u64> = parser
                    .buffer
                    .chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .collect();
                let pattern: String = stack
                    .iter()
                    .map(|&a| if is_kernel(a) { 'K' } else { 'U' })
                    .collect();
                println!(
                    "STACK   ts={:>16} tid={:<6} n={:<4} leaf->root: {}",
                    ts,
                    tid,
                    stack.len(),
                    pattern
                );
                printed += 1;
            }
            _ => {}
        }
    })
    .unwrap();
}
