// Prototype to answer: do *non-sample* stackwalk-bearing events (e.g. DiskIo,
// and other events that would become markers) get their user stack delivered at
// their own timestamp, or is the user portion deferred to a later user-mode
// return (like sampling-profiler kernel samples)?
//
// For every StackWalk fragment we record (StackThread, StackProcess,
// EventTimeStamp) and whether the fragment is kernel-rooted (outermost frame in
// the kernel) or user-rooted. We bucket each (tid, ts) group by what event sits
// at that exact (tid, ts):
//   - "sample"  : a PerfInfo/SampleProf event
//   - "cswitch" : a Thread/CSwitch event
//   - "other"   : anything else (these are the marker-like events)
//
// For the "other" kernel-only groups (the interesting ones), we then report:
//   - which processes / threads they're on (is it really pid 4 / System?), and
//   - whether a later user-rooted fragment exists on the same thread that a
//     sample-style "deferred user stack" sweep would attach (i.e. the next
//     user-rooted group at ts > T, skipping intervening kernel-rooted ones).
//
// Usage: cargo run --release --example marker-stacks -- <path-to-etl>

use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;
use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;

fn is_kernel(ip: u64) -> bool {
    ip >= 0xFFFF000000000000
}

#[derive(Default, Clone)]
struct Group {
    has_kernel_rooted: bool,
    has_user_rooted: bool,
    pid: u32,
}

#[derive(Default)]
struct Shapes {
    kernel_only: u64,
    user_only: u64,
    both: u64,
}
impl Shapes {
    fn add(&mut self, g: &Group) {
        match (g.has_kernel_rooted, g.has_user_rooted) {
            (true, false) => self.kernel_only += 1,
            (false, true) => self.user_only += 1,
            (true, true) => self.both += 1,
            (false, false) => {}
        }
    }
    fn report(&self, label: &str) {
        let total = self.kernel_only + self.user_only + self.both;
        println!("  {label}: {total} groups");
        println!("     kernel-only (user deferred?): {}", self.kernel_only);
        println!("     user-only                   : {}", self.user_only);
        println!("     both (k+u at own ts)        : {}", self.both);
    }
}

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);

    let mut sample_ts: HashMap<(u32, u64), ()> = HashMap::new();
    let mut cswitch_ts: HashMap<(u32, u64), ()> = HashMap::new();
    let mut other_ts: HashMap<(u32, u64), String> = HashMap::new();
    let mut process_names: HashMap<u32, String> = HashMap::new();
    // (tid, ts) -> accumulated fragment shape
    let mut groups: HashMap<(u32, u64), Group> = HashMap::new();

    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };
        let name = s.name().to_owned();
        let header_ts = e.EventHeader.TimeStamp as u64;
        let header_tid = e.EventHeader.ThreadId;

        match name.as_str() {
            "MSNT_SystemTrace/StackWalk/Stack" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("StackThread");
                let pid: u32 = parser.parse("StackProcess");
                let ts: u64 = parser.parse("EventTimeStamp");
                let stack: Vec<u64> = parser
                    .buffer
                    .chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .collect();
                if stack.is_empty() {
                    return;
                }
                let kernel_rooted = is_kernel(*stack.last().unwrap());
                let g = groups.entry((tid, ts)).or_default();
                g.pid = pid;
                if kernel_rooted {
                    g.has_kernel_rooted = true;
                } else {
                    g.has_user_rooted = true;
                }
            }
            "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("ThreadId");
                sample_ts.insert((tid, header_ts), ());
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("NewThreadId");
                cswitch_ts.insert((tid, header_ts), ());
            }
            "MSNT_SystemTrace/Process/Start"
            | "MSNT_SystemTrace/Process/DCStart"
            | "MSNT_SystemTrace/Process/DCEnd" => {
                let mut parser = Parser::create(&s);
                let pid: u32 = parser.parse("ProcessId");
                let image: String = parser.parse("ImageFileName");
                process_names.insert(pid, image);
                other_ts.insert((header_tid, header_ts), name);
            }
            _ => {
                other_ts.insert((header_tid, header_ts), name);
            }
        }
    })
    .unwrap();

    let mut sample = Shapes::default();
    let mut cswitch = Shapes::default();
    let mut other = Shapes::default();

    for (key, g) in &groups {
        if sample_ts.contains_key(key) {
            sample.add(g);
        } else if cswitch_ts.contains_key(key) {
            cswitch.add(g);
        } else if other_ts.contains_key(key) {
            other.add(g);
        }
    }

    println!("== StackWalk group shapes by associated event ==");
    sample.report("sample (PerfInfo/SampleProf)");
    cswitch.report("cswitch (Thread/CSwitch)");
    other.report("other (marker-like events)");

    // Per-thread timeline of group shapes, for the deferred-availability check.
    let mut per_tid: BTreeMap<u32, BTreeMap<u64, Group>> = BTreeMap::new();
    for ((tid, ts), g) in &groups {
        per_tid.entry(*tid).or_default().insert(*ts, g.clone());
    }

    // Analyze the "other" kernel-only groups.
    let mut by_process: HashMap<String, u64> = HashMap::new();
    let mut pid4 = 0u64;
    let mut non_pid4 = 0u64;
    let mut deferred_user_available = 0u64; // a later user-rooted frag exists on this thread
    let mut no_later_user = 0u64;
    let mut deltas: Vec<u64> = Vec::new();

    for ((tid, ts), g) in &groups {
        let is_other = !sample_ts.contains_key(&(*tid, *ts))
            && !cswitch_ts.contains_key(&(*tid, *ts))
            && other_ts.contains_key(&(*tid, *ts));
        if !(is_other && g.has_kernel_rooted && !g.has_user_rooted) {
            continue;
        }

        if g.pid == 4 {
            pid4 += 1;
        } else {
            non_pid4 += 1;
        }
        let pname = process_names
            .get(&g.pid)
            .cloned()
            .unwrap_or_else(|| format!("pid {}", g.pid));
        *by_process.entry(pname).or_default() += 1;

        // Would a sample-style sweep find a user stack? Look for the next
        // user-rooted group on this thread at ts > T (skipping kernel-rooted ones).
        let tl = &per_tid[tid];
        match tl.range((*ts + 1)..).find(|(_, ng)| ng.has_user_rooted) {
            Some((next_ts, _)) => {
                deferred_user_available += 1;
                deltas.push(next_ts - ts);
            }
            None => no_later_user += 1,
        }
    }

    println!("\n== 'other' KERNEL-ONLY groups: which process? ==");
    println!("  on pid 4 (System): {pid4}");
    println!("  on other pids    : {non_pid4}");
    let mut v: Vec<_> = by_process.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (name, count) in v.into_iter().take(15) {
        println!("  {count:>8}  {name}");
    }

    deltas.sort();
    let median = deltas.get(deltas.len() / 2).copied().unwrap_or(0);
    let pct = |p: usize| deltas.get(deltas.len() * p / 100).copied().unwrap_or(0);
    println!("\n== Would a deferred-user-stack sweep attach a user stack? ==");
    println!("  later user-rooted fragment exists on same thread: {deferred_user_available}");
    println!("  no later user-rooted fragment on that thread    : {no_later_user}");
    println!(
        "  ts-delta to that next user stack: p50={} p90={} (raw timestamp units)",
        median,
        pct(90)
    );
}
