// Stats tool to understand how ETW StackWalk fragments are shaped and how they
// associate with samples. Answers questions like:
//   - How are stack fragments ordered (is index 0 the leaf or the root)?
//   - How often is a single fragment "mixed" (contains both kernel and user frames)?
//   - In the kernel->user->kernel (KeUserModeCallback) case, how do the frames
//     get split across fragments?
//   - How many pending (kernel-only) samples does a single user stack finalize?
//   - How often is a userspace stack apparently "missing"?
//
// Usage: cargo run --release --example stack-stats -- <path-to-etl>

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::TryInto;
use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;

fn is_kernel_address(ip: u64, pointer_size: u32) -> bool {
    if pointer_size == 4 {
        return ip >= 0x80000000;
    }
    ip >= 0xFFFF000000000000
}

struct Event {
    timestamp: i64,
    thread_id: u32,
    has_kernel_stack: bool,
}

struct ThreadState {
    // number of consecutive kernel-only samples awaiting a user stack
    pending_kernel_samples: u32,
    // timestamp of the most recent pending kernel sample
    last_pending_ts: Option<i64>,
}
impl ThreadState {
    fn new() -> Self {
        ThreadState {
            pending_kernel_samples: 0,
            last_pending_ts: None,
        }
    }
}

#[derive(Default)]
struct Stats {
    fragments: u64,
    // fragment shape, classified by (first_frame_is_kernel, last_frame_is_kernel)
    first_user_last_user: u64,
    first_user_last_kernel: u64,
    first_kernel_last_user: u64,
    first_kernel_last_kernel: u64,
    // fragments that contain BOTH a kernel and a user frame
    mixed_fragments: u64,
    // histogram of the number of mode transitions within a single fragment
    // (0 = pure, 1 = one boundary e.g. K..U, 2 = K..U..K, etc.)
    transitions_hist: HashMap<u32, u64>,
    // how many fragments classified as "kernel" arrived for a sample before its
    // user stack (i.e. KeUserModeCallback concatenation). Indexed by count.
    multi_kernel_per_sample: HashMap<u32, u64>,
    // histogram of how many pending kernel samples a single user stack finalized
    pending_finalized_hist: HashMap<u32, u64>,
    // user stack arrived but there were no matching pending samples
    user_stack_no_pending: u64,
    // apparent missing-userspace situations (a kernel sample never got its own
    // user stack; a later user stack is being substituted)
    missing_userspace: u64,
}

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);

    let mut events: Vec<Event> = Vec::new();
    let mut threads: HashMap<u32, ThreadState> = HashMap::new();
    let mut stats = Stats::default();
    // per (thread,timestamp) count of kernel fragments seen, to detect concatenation
    let mut kernel_frag_count: HashMap<(u32, i64), u32> = HashMap::new();

    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };
        let mut thread_id = e.EventHeader.ThreadId;
        match s.name() {
            "MSNT_SystemTrace/StackWalk/Stack" => {
                let mut parser = Parser::create(&s);
                let thread_id: u32 = parser.parse("StackThread");
                let timestamp: u64 = parser.parse("EventTimeStamp");

                let stack: Vec<u64> = parser
                    .buffer
                    .chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .collect();
                if stack.is_empty() {
                    return;
                }

                stats.fragments += 1;

                let first_kernel = is_kernel_address(*stack.first().unwrap(), 8);
                let last_kernel = is_kernel_address(*stack.last().unwrap(), 8);
                match (first_kernel, last_kernel) {
                    (false, false) => stats.first_user_last_user += 1,
                    (false, true) => stats.first_user_last_kernel += 1,
                    (true, false) => stats.first_kernel_last_user += 1,
                    (true, true) => stats.first_kernel_last_kernel += 1,
                }

                let mut transitions = 0;
                let mut has_kernel = false;
                let mut has_user = false;
                for w in stack.windows(2) {
                    let a = is_kernel_address(w[0], 8);
                    let b = is_kernel_address(w[1], 8);
                    if a != b {
                        transitions += 1;
                    }
                }
                for &addr in &stack {
                    if is_kernel_address(addr, 8) {
                        has_kernel = true;
                    } else {
                        has_user = true;
                    }
                }
                if has_kernel && has_user {
                    stats.mixed_fragments += 1;
                }
                *stats.transitions_hist.entry(transitions).or_default() += 1;

                // Use the log-stacks.rs convention: a fragment that ends in a
                // kernel address is treated as a (partial) kernel stack.
                let ends_in_kernel = last_kernel;

                let thread = match threads.entry(thread_id) {
                    Entry::Occupied(e) => e.into_mut(),
                    Entry::Vacant(e) => e.insert(ThreadState::new()),
                };

                if ends_in_kernel {
                    let c = kernel_frag_count
                        .entry((thread_id, timestamp as i64))
                        .or_default();
                    *c += 1;
                    if *c == 1 {
                        thread.pending_kernel_samples += 1;
                    }
                    thread.last_pending_ts = Some(timestamp as i64);
                } else {
                    // user stack: finalizes all pending kernel samples
                    *stats
                        .pending_finalized_hist
                        .entry(thread.pending_kernel_samples)
                        .or_default() += 1;
                    if thread.pending_kernel_samples == 0 {
                        stats.user_stack_no_pending += 1;
                    }
                    if let Some(last_ts) = thread.last_pending_ts {
                        if last_ts < timestamp as i64 {
                            stats.missing_userspace += 1;
                        }
                    }
                    thread.pending_kernel_samples = 0;
                    thread.last_pending_ts = None;
                }
            }
            "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                let mut parser = Parser::create(&s);
                thread_id = parser.parse("ThreadId");
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                let mut parser = Parser::create(&s);
                thread_id = parser.parse("NewThreadId");
            }
            _ => {}
        }

        events.push(Event {
            timestamp: e.EventHeader.TimeStamp,
            thread_id,
            has_kernel_stack: false,
        });
    })
    .unwrap();

    // tally multi-kernel-per-sample
    for (_k, c) in &kernel_frag_count {
        *stats.multi_kernel_per_sample.entry(*c).or_default() += 1;
    }

    let _ = &events; // silence unused field warnings
    let _ = Event {
        timestamp: 0,
        thread_id: 0,
        has_kernel_stack: false,
    };

    println!("== Fragment shapes (n = {}) ==", stats.fragments);
    println!(
        "  first=USER  last=USER  : {:>10}",
        stats.first_user_last_user
    );
    println!(
        "  first=USER  last=KERN  : {:>10}",
        stats.first_user_last_kernel
    );
    println!(
        "  first=KERN  last=USER  : {:>10}",
        stats.first_kernel_last_user
    );
    println!(
        "  first=KERN  last=KERN  : {:>10}",
        stats.first_kernel_last_kernel
    );
    println!("  mixed (both K and U)   : {:>10}", stats.mixed_fragments);

    println!("\n== Mode transitions within a single fragment ==");
    let mut keys: Vec<_> = stats.transitions_hist.keys().cloned().collect();
    keys.sort();
    for k in keys {
        println!("  {k} transitions: {:>10}", stats.transitions_hist[&k]);
    }

    println!("\n== Kernel fragments per (thread,timestamp) [concatenation] ==");
    let mut keys: Vec<_> = stats.multi_kernel_per_sample.keys().cloned().collect();
    keys.sort();
    for k in keys {
        println!(
            "  {k} kernel fragment(s): {:>10}",
            stats.multi_kernel_per_sample[&k]
        );
    }

    println!("\n== Pending kernel samples finalized by one user stack ==");
    let mut keys: Vec<_> = stats.pending_finalized_hist.keys().cloned().collect();
    keys.sort();
    for k in keys {
        println!(
            "  user stack finalized {k} pending sample(s): {:>10}",
            stats.pending_finalized_hist[&k]
        );
    }
    println!(
        "  (of those, user stacks with no pending sample: {})",
        stats.user_stack_no_pending
    );

    println!(
        "\n== Apparent missing-userspace situations: {} ==",
        stats.missing_userspace
    );
}
