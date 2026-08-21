// Pick specific kernel-only marker-like events on *user* threads that have a
// later user-rooted stack, and dump everything that happens on that thread
// between the marker and that next user stack. The point: decide whether the
// "next user stack" is the genuine deferred user stack (thread was blocked in
// the I/O the whole time -> only context switches in between) or unrelated later
// activity (thread ran other user code in between -> user-mode samples appear).
//
// Usage: cargo run --release --example marker-stack-inspect -- <path-to-etl> [N]

use std::collections::{BTreeMap, HashMap, HashSet};
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

struct Candidate {
    tid: u32,
    pid: u32,
    pname: String,
    event_name: String,
    t_marker: u64,
    t_next_user: u64,
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let n_candidates: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);

    // ---- Pass 1: find candidates ----
    let mut sample_ts: HashSet<(u32, u64)> = HashSet::new();
    let mut cswitch_ts: HashSet<(u32, u64)> = HashSet::new();
    let mut other_ts: HashMap<(u32, u64), String> = HashMap::new();
    let mut process_names: HashMap<u32, String> = HashMap::new();
    let mut groups: HashMap<(u32, u64), Group> = HashMap::new();

    open_trace(Path::new(&path), |e| {
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };
        let name = s.name();
        let header_ts = e.EventHeader.TimeStamp as u64;
        let header_tid = e.EventHeader.ThreadId;
        match name {
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
                sample_ts.insert((tid, header_ts));
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("NewThreadId");
                cswitch_ts.insert((tid, header_ts));
            }
            "MSNT_SystemTrace/Process/Start"
            | "MSNT_SystemTrace/Process/DCStart"
            | "MSNT_SystemTrace/Process/DCEnd" => {
                let mut parser = Parser::create(&s);
                let pid: u32 = parser.parse("ProcessId");
                let image: String = parser.parse("ImageFileName");
                process_names.insert(pid, image);
                other_ts.insert((header_tid, header_ts), name.to_owned());
            }
            _ => {
                other_ts.insert((header_tid, header_ts), name.to_owned());
            }
        }
    })
    .unwrap();

    let mut per_tid: HashMap<u32, BTreeMap<u64, Group>> = HashMap::new();
    for ((tid, ts), g) in &groups {
        per_tid.entry(*tid).or_default().insert(*ts, g.clone());
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for ((tid, ts), g) in &groups {
        let is_other = !sample_ts.contains(&(*tid, *ts))
            && !cswitch_ts.contains(&(*tid, *ts))
            && other_ts.contains_key(&(*tid, *ts));
        if !(is_other && g.has_kernel_rooted && !g.has_user_rooted && g.pid != 4) {
            continue;
        }
        if let Some((next_ts, _)) = per_tid[tid].range((*ts + 1)..).find(|(_, ng)| ng.has_user_rooted)
        {
            candidates.push(Candidate {
                tid: *tid,
                pid: g.pid,
                pname: process_names
                    .get(&g.pid)
                    .cloned()
                    .unwrap_or_else(|| format!("pid {}", g.pid)),
                event_name: other_ts.get(&(*tid, *ts)).cloned().unwrap_or_default(),
                t_marker: *ts,
                t_next_user: *next_ts,
            });
        }
    }
    // Sort by gap. Keep the N smallest and N largest, so we see both the
    // plausibly-genuine deferrals (small gap) and the suspicious ones (huge gap).
    candidates.sort_by_key(|c| c.t_next_user - c.t_marker);
    if candidates.len() > 2 * n_candidates {
        let largest: Vec<_> = candidates.split_off(candidates.len() - n_candidates);
        candidates.truncate(n_candidates);
        candidates.extend(largest);
    }

    let target_tids: HashSet<u32> = candidates.iter().map(|c| c.tid).collect();
    println!("Selected {} candidate(s) (largest marker->next-user gap):", candidates.len());
    for c in &candidates {
        println!(
            "  tid={} {} ({}) {} gap={}",
            c.tid,
            c.pname,
            c.pid,
            c.event_name,
            c.t_next_user - c.t_marker
        );
    }
    if candidates.is_empty() {
        return;
    }

    // ---- Pass 2: record the timeline on the target threads ----
    // (ts, tid) -> description of what happened
    let mut timeline: Vec<(u64, u32, String)> = Vec::new();
    let mut record = |ts: u64, tid: u32, desc: String, target: &HashSet<u32>| {
        if target.contains(&tid) {
            timeline.push((ts, tid, desc));
        }
    };

    open_trace(Path::new(&path), |e| {
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };
        let name = s.name();
        let header_ts = e.EventHeader.TimeStamp as u64;
        let header_tid = e.EventHeader.ThreadId;
        match name {
            "MSNT_SystemTrace/StackWalk/Stack" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("StackThread");
                let ts: u64 = parser.parse("EventTimeStamp");
                let stack: Vec<u64> = parser
                    .buffer
                    .chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()))
                    .collect();
                if stack.is_empty() {
                    return;
                }
                let root = if is_kernel(*stack.last().unwrap()) {
                    "KERNEL-rooted"
                } else {
                    "USER-rooted"
                };
                record(
                    ts,
                    tid,
                    format!("StackWalk {root} ({} frames)", stack.len()),
                    &target_tids,
                );
            }
            "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                let mut parser = Parser::create(&s);
                let tid: u32 = parser.parse("ThreadId");
                let ip: u64 = parser.try_parse("InstructionPointer").unwrap_or(0);
                let mode = if is_kernel(ip) { "kernel" } else { "USER" };
                record(header_ts, tid, format!("SampleProf (ip in {mode})"), &target_tids);
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                let mut parser = Parser::create(&s);
                let old_tid: u32 = parser.parse("OldThreadId");
                let new_tid: u32 = parser.parse("NewThreadId");
                let wait: i8 = parser.try_parse("OldThreadWaitReason").unwrap_or(-1);
                record(header_ts, old_tid, format!("CSwitch OUT (wait_reason={wait})"), &target_tids);
                record(header_ts, new_tid, "CSwitch IN".to_string(), &target_tids);
            }
            _ => {
                record(header_ts, header_tid, format!("event {name}"), &target_tids);
            }
        }
    })
    .unwrap();

    timeline.sort_by_key(|(ts, _, _)| *ts);

    for c in &candidates {
        println!(
            "\n================ tid={} {} — {} @ {} -> next user stack @ {} (gap {}) ================",
            c.tid, c.pname, c.event_name, c.t_marker, c.t_next_user, c.t_next_user - c.t_marker
        );
        for (ts, tid, desc) in &timeline {
            if *tid == c.tid && *ts >= c.t_marker && *ts <= c.t_next_user {
                let marker = if *ts == c.t_marker {
                    "  <-- MARKER (kernel-only)"
                } else if *ts == c.t_next_user {
                    "  <-- next USER stack"
                } else {
                    ""
                };
                println!("  +{:>10}  {}{}", ts - c.t_marker, desc, marker);
            }
        }
    }
}
