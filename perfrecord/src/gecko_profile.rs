use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

use std::cmp::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct ProfileBuilder {
    pid: u32,
    interval: Duration,
    libs: Vec<Lib>,
    threads: HashMap<u32, ThreadBuilder>,
    start_time: Instant,
    end_time: Option<Instant>,
    command_name: String,
    subprocesses: Vec<ProfileBuilder>,
}

impl ProfileBuilder {
    pub fn new(start_time: Instant, command_name: &str, pid: u32, interval: Duration) -> Self {
        ProfileBuilder {
            pid,
            interval,
            threads: HashMap::new(),
            libs: Vec::new(),
            start_time,
            end_time: None,
            command_name: command_name.to_owned(),
            subprocesses: Vec::new(),
        }
    }

    pub fn set_end_time(&mut self, end_time: Instant) {
        self.end_time = Some(end_time);
    }

    pub fn add_lib(
        &mut self,
        name: &str,
        path: &str,
        uuid: &Uuid,
        arch: &'static str,
        address_range: &std::ops::Range<u64>,
    ) {
        self.libs.push(Lib {
            name: name.to_string(),
            path: path.to_string(),
            arch,
            breakpad_id: format!("{:X}0", uuid.to_simple()),
            start_address: address_range.start,
            end_address: address_range.end,
        })
    }

    // pub fn add_sample(&mut self, thread_index: u32, timestamp: f64, frames: &[u64]) {
    //     let thread = self
    //         .threads
    //         .entry(thread_index)
    //         .or_insert_with(|| ThreadBuilder::new(thread_index, timestamp));
    //     thread.add_sample(timestamp, frames);
    // }

    pub fn add_thread(&mut self, thread_builder: ThreadBuilder) {
        self.threads.insert(thread_builder.index, thread_builder);
    }

    pub fn add_subprocess(&mut self, profile_builder: ProfileBuilder) {
        self.subprocesses.push(profile_builder);
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut sorted_threads: Vec<_> = self.threads.iter().collect();
        sorted_threads.sort_by(|(_, a), (_, b)| {
            if let Some(ordering) = a.get_start_time().partial_cmp(&b.get_start_time()) {
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            let ordering = a.get_name().cmp(&b.get_name());
            if ordering != Ordering::Equal {
                return ordering;
            }
            a.get_tid().cmp(&b.get_tid())
        });
        let threads: Vec<Value> = sorted_threads
            .into_iter()
            .map(|(_, thread)| thread.to_json(&self.command_name, self.start_time))
            .collect();
        let mut sorted_libs: Vec<_> = self.libs.iter().collect();
        sorted_libs.sort_by_key(|l| l.start_address);
        let libs: Vec<Value> = sorted_libs.iter().map(|l| l.to_json()).collect();

        let mut sorted_subprocesses: Vec<_> = self.subprocesses.iter().collect();
        sorted_subprocesses.sort_by(|a, b| {
            if let Some(ordering) = a.start_time.partial_cmp(&b.start_time) {
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            a.pid.cmp(&b.pid)
        });

        let subprocesses: Vec<Value> = sorted_subprocesses.iter().map(|p| p.to_json()).collect();

        let start_time_system = instant_to_system_time(self.start_time);
        let duration_since_unix_epoch = start_time_system.duration_since(UNIX_EPOCH).unwrap();
        let start_time_ms_since_unix_epoch = duration_since_unix_epoch.as_secs_f64() * 1000.0;

        let end_time_ms_since_start = self
            .end_time
            .map(|end_time| to_profile_timestamp(end_time, self.start_time));

        json!({
            "meta": {
                "version": 14,
                "startTime": start_time_ms_since_unix_epoch,
                "shutdownTime": end_time_ms_since_start,
                "pausedRanges": [],
                "product": self.command_name,
                "interval": self.interval.as_secs_f64() * 1000.0,
                "pid": self.pid,
                "processType": 0,
                "categories": [
                    {
                        "name": "Regular",
                        "color": "blue",
                    },
                    {
                        "name": "Other",
                        "color": "grey",
                    }
                ],
                "sampleUnits": {
                    "time": "ms",
                    "eventDelay": "ms",
                    "threadCPUDelta": "µs"
                }
            },
            "libs": libs,
            "threads": threads,
            "processes": subprocesses,
        })
    }
}

fn instant_to_system_time(instant: Instant) -> SystemTime {
    let now_instant = Instant::now();
    let now_system = SystemTime::now();
    let duration_before_now = now_instant - instant;
    now_system - duration_before_now
}

fn to_profile_timestamp(instant: Instant, process_start: Instant) -> f64 {
    (instant - process_start).as_secs_f64() * 1000.0
}

#[derive(Debug)]
pub struct ThreadBuilder {
    pid: u32,
    index: u32,
    name: Option<String>,
    start_time: Instant,
    end_time: Option<Instant>,
    is_main: bool,
    is_libdispatch_thread: bool,
    stack_table: StackTable,
    frame_table: FrameTable,
    samples: SampleTable,
    string_table: StringTable,
}

impl ThreadBuilder {
    pub fn new(
        pid: u32,
        thread_index: u32,
        start_time: Instant,
        is_main: bool,
        is_libdispatch_thread: bool,
    ) -> Self {
        ThreadBuilder {
            pid,
            index: thread_index,
            name: None,
            start_time,
            end_time: None,
            is_main,
            is_libdispatch_thread,
            stack_table: StackTable::new(),
            frame_table: FrameTable::new(),
            samples: SampleTable(Vec::new()),
            string_table: StringTable::new(),
        }
    }

    pub fn get_start_time(&self) -> Instant {
        self.start_time
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = Some(name.to_owned());
    }

    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn get_tid(&self) -> u32 {
        self.index
    }

    pub fn add_sample(
        &mut self,
        timestamp: Instant,
        frames: &[u64],
        cpu_delta: Duration,
    ) -> Option<usize> {
        let stack_index = self.stack_index_for_frames(frames);
        self.samples.0.push(Sample {
            timestamp,
            stack_index,
            cpu_delta_us: cpu_delta.as_micros() as u64,
        });
        stack_index
    }

    pub fn add_sample_same_stack(
        &mut self,
        timestamp: Instant,
        previous_stack: Option<usize>,
        cpu_delta: Duration,
    ) {
        self.samples.0.push(Sample {
            timestamp,
            stack_index: previous_stack,
            cpu_delta_us: cpu_delta.as_micros() as u64,
        });
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        self.end_time = Some(end_time);
    }

    fn stack_index_for_frames(&mut self, frames: &[u64]) -> Option<usize> {
        let frame_indexes: Vec<_> = frames
            .iter()
            .map(|&address| self.frame_index_for_address(address))
            .collect();
        self.stack_table.index_for_frames(&frame_indexes)
    }

    fn frame_index_for_address(&mut self, address: u64) -> usize {
        self.frame_table
            .index_for_frame(&mut self.string_table, address)
    }

    fn to_json(&self, process_name: &str, process_start: Instant) -> Value {
        let name = if self.is_main {
            // https://github.com/firefox-devtools/profiler/issues/2508
            "GeckoMain".to_string()
        } else if let Some(name) = &self.name {
            name.clone()
        } else if self.is_libdispatch_thread {
            "libdispatch".to_string()
        } else {
            format!("Thread <{}>", self.index)
        };
        let register_time = to_profile_timestamp(self.start_time, process_start);
        let unregister_time = self
            .end_time
            .map(|end_time| to_profile_timestamp(end_time, process_start));
        json!({
            "name": name,
            "tid": self.index,
            "pid": self.pid,
            "processType": "default",
            "processName": process_name,
            "registerTime": register_time,
            "unregisterTime": unregister_time,
            "frameTable": self.frame_table.to_json(),
            "stackTable": self.stack_table.to_json(),
            "samples": self.samples.to_json(process_start),
            "markers": {
                "schema": {
                    "name": 0,
                    "time": 1,
                    "data": 2
                },
                "data": []
            },
            "stringTable": self.string_table.to_json()
        })
    }
}

#[derive(Debug)]
struct Lib {
    name: String,
    path: String,
    arch: &'static str,
    breakpad_id: String,
    start_address: u64,
    end_address: u64,
}

impl Lib {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "debugName": self.name,
            "path": self.path,
            "debugPath": self.path,
            "breakpadId": self.breakpad_id,
            "offset": 0,
            "start": self.start_address,
            "end": self.end_address,
            "arch": self.arch,
        })
    }
}

#[derive(Debug)]
struct StackTable {
    // (parent stack, frame_index)
    stacks: Vec<(Option<usize>, usize)>,

    // (parent stack, frame_index) -> stack index
    index: BTreeMap<(Option<usize>, usize), usize>,
}

impl StackTable {
    pub fn new() -> StackTable {
        StackTable {
            stacks: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    pub fn index_for_frames(&mut self, frame_indexes: &[usize]) -> Option<usize> {
        let mut prefix = None;
        for &frame_index in frame_indexes {
            match self.index.get(&(prefix, frame_index)) {
                Some(stack_index) => {
                    prefix = Some(*stack_index);
                }
                None => {
                    let stack_index = self.stacks.len();
                    self.stacks.push((prefix, frame_index));
                    self.index.insert((prefix, frame_index), stack_index);
                    prefix = Some(stack_index);
                }
            }
        }
        prefix
    }

    pub fn to_json(&self) -> Value {
        let data: Vec<Value> = self
            .stacks
            .iter()
            .map(|(prefix, frame_index)| {
                let prefix = match prefix {
                    Some(prefix) => Value::Number((*prefix as u64).into()),
                    None => Value::Null,
                };
                json!([prefix, frame_index])
            })
            .collect();
        json!({
            "schema": {
                "prefix": 0,
                "frame": 1,
            },
            "data": data
        })
    }
}

#[derive(Debug)]
struct FrameTable {
    // [string_index]
    frames: Vec<usize>,

    // address -> frame index
    index: BTreeMap<u64, usize>,
}

impl FrameTable {
    pub fn new() -> FrameTable {
        FrameTable {
            frames: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    pub fn index_for_frame(&mut self, string_table: &mut StringTable, address: u64) -> usize {
        let frames = &mut self.frames;
        *self.index.entry(address).or_insert_with(|| {
            let frame_index = frames.len();
            let location_string = format!("0x{:x}", address);
            let location_string_index = string_table.index_for_string(&location_string);
            frames.push(location_string_index);
            frame_index
        })
    }

    pub fn to_json(&self) -> Value {
        let data: Vec<Value> = self
            .frames
            .iter()
            .map(|location| {
                let category = 0;
                json!([*location, false, null, null, null, null, category])
            })
            .collect();
        json!({
            "schema": {
                "location": 0,
                "relevantForJS": 1,
                "implementation": 2,
                "optimizations": 3,
                "line": 4,
                "column": 5,
                "category": 6,
            },
            "data": data
        })
    }
}

#[derive(Debug)]
struct SampleTable(Vec<Sample>);

impl SampleTable {
    pub fn to_json(&self, process_start: Instant) -> Value {
        let data: Vec<Value> = self
            .0
            .iter()
            .map(|sample| {
                json!([
                    sample.stack_index,
                    to_profile_timestamp(sample.timestamp, process_start),
                    0.0,
                    sample.cpu_delta_us
                ])
            })
            .collect();
        json!({
            "schema": {
                "stack": 0,
                "time": 1,
                "eventDelay": 2,
                "threadCPUDelta": 3
            },
            "data": data
        })
    }
}

#[derive(Debug)]
struct Sample {
    timestamp: Instant,
    stack_index: Option<usize>,
    cpu_delta_us: u64,
}

#[derive(Debug)]
struct StringTable {
    strings: Vec<String>,
    index: HashMap<String, usize>,
}

impl StringTable {
    pub fn new() -> Self {
        StringTable {
            strings: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn index_for_string(&mut self, s: &str) -> usize {
        match self.index.get(s) {
            Some(string_index) => *string_index,
            None => {
                let string_index = self.strings.len();
                self.strings.push(s.to_string());
                self.index.insert(s.to_string(), string_index);
                string_index
            }
        }
    }

    pub fn to_json(&self) -> Value {
        Value::Array(
            self.strings
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        )
    }
}
