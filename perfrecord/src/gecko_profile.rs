use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use std::cmp::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct ProfileBuilder {
    pid: u32,
    interval: Duration,
    libs: Vec<Lib>,
    threads: HashMap<u32, ThreadBuilder>,
    start_time: f64,       // as milliseconds since unix epoch
    end_time: Option<f64>, // as milliseconds since start_time
    command_name: String,
    subprocesses: Vec<ProfileBuilder>,
}

impl ProfileBuilder {
    pub fn new(start_time: Instant, command_name: &str, pid: u32, interval: Duration) -> Self {
        let now_instant = Instant::now();
        let now_system = SystemTime::now();
        let duration_before_now = now_instant.duration_since(start_time);
        let start_time_system = now_system - duration_before_now;
        let duration_since_unix_epoch = start_time_system.duration_since(UNIX_EPOCH).unwrap();
        ProfileBuilder {
            pid,
            interval,
            threads: HashMap::new(),
            libs: Vec::new(),
            start_time: duration_since_unix_epoch.as_secs_f64() * 1000.0,
            end_time: None,
            command_name: command_name.to_owned(),
            subprocesses: Vec::new(),
        }
    }

    pub fn set_end_time(&mut self, duration_since_start: Duration) {
        self.end_time = Some(duration_since_start.as_secs_f64() * 1000.0);
    }

    pub fn add_lib(
        &mut self,
        name: &str,
        path: &str,
        uuid: &Uuid,
        address_range: &std::ops::Range<u64>,
    ) {
        self.libs.push(Lib {
            name: name.to_string(),
            path: path.to_string(),
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
            .map(|(_, thread)| thread.to_json())
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
        json!({
            "meta": {
                "version": 14,
                "startTime": self.start_time,
                "shutdownTime": self.end_time,
                "pausedRanges": [],
                "product": self.command_name,
                "interval": self.interval.as_secs_f64() * 1000.0,
                "pid": self.pid,
                "processType": 0,
                "categories": [
                    {
                        "name": "Idle",
                        "color": "transparent",
                    },
                    {
                        "name": "Running",
                        "color": "blue",
                    },
                    {
                        "name": "Other",
                        "color": "grey",
                    }
                ]
            },
            "libs": libs,
            "threads": threads,
            "processes": subprocesses,
        })
    }
}

#[derive(Debug)]
pub struct ThreadBuilder {
    pid: u32,
    index: u32,
    name: Option<String>,
    start_time: f64,
    end_time: Option<f64>,
    stack_table: StackTable,
    frame_table: FrameTable,
    samples: SampleTable,
    string_table: StringTable,
}

impl ThreadBuilder {
    pub fn new(pid: u32, thread_index: u32, start_time: f64) -> Self {
        ThreadBuilder {
            pid,
            index: thread_index,
            name: None,
            start_time,
            end_time: None,
            stack_table: StackTable::new(),
            frame_table: FrameTable::new(),
            samples: SampleTable(Vec::new()),
            string_table: StringTable::new(),
        }
    }

    pub fn get_start_time(&self) -> f64 {
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

    pub fn add_sample(&mut self, timestamp: f64, frames: &[u64], idle: bool) {
        let stack_index = self.stack_index_for_frames(frames, idle);
        self.samples.0.push(Sample {
            timestamp,
            stack_index,
        });
    }

    pub fn notify_dead(&mut self, end_time: f64) {
        self.end_time = Some(end_time);
    }

    fn stack_index_for_frames(&mut self, frames: &[u64], idle: bool) -> Option<usize> {
        let frame_indexes: Vec<_> = frames
            .iter()
            .enumerate()
            .map(|(i, &address)| {
                let frame_idle = idle && i == frames.len() - 1;
                self.frame_index_for_address(address, frame_idle)
            })
            .collect();
        self.stack_table.index_for_frames(&frame_indexes)
    }

    fn frame_index_for_address(&mut self, address: u64, idle: bool) -> usize {
        let location_string = format!("0x{:x}", address);
        let location_string_index = self.string_table.index_for_string(&location_string);
        self.frame_table
            .index_for_frame(location_string_index, idle)
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name.clone().unwrap_or_else(|| format!("Thread <{}>", self.index)),
            "tid": self.index,
            "pid": self.pid,
            "processType": "default",
            "registerTime": self.start_time,
            "unregisterTime": self.end_time,
            "frameTable": self.frame_table.to_json(),
            "stackTable": self.stack_table.to_json(),
            "samples": self.samples.to_json(),
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
            "arch": "x86_64"
        })
    }
}

#[derive(Debug)]
struct StackTable {
    stacks: Vec<(Option<usize>, usize)>,
    index: HashMap<(Option<usize>, usize), usize>,
}

impl StackTable {
    pub fn new() -> StackTable {
        StackTable {
            stacks: Vec::new(),
            index: HashMap::new(),
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
    frames: Vec<(usize, bool)>,
    index: HashMap<(usize, bool), usize>,
}

impl FrameTable {
    pub fn new() -> FrameTable {
        FrameTable {
            frames: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn index_for_frame(&mut self, location_string_index: usize, idle: bool) -> usize {
        match self.index.get(&(location_string_index, idle)) {
            Some(frame_index) => *frame_index,
            None => {
                let frame_index = self.frames.len();
                self.frames.push((location_string_index, idle));
                self.index
                    .insert((location_string_index, idle), frame_index);
                frame_index
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let data: Vec<Value> = self
            .frames
            .iter()
            .map(|&(location, idle)| {
                let category = if idle { 0 } else { 1 };
                json!([location, false, null, null, null, null, category])
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
    pub fn to_json(&self) -> Value {
        let data: Vec<Value> = self
            .0
            .iter()
            .map(|sample| json!([sample.stack_index, sample.timestamp]))
            .collect();
        json!({
            "schema": {
                "stack": 0,
                "time": 1,
                "responsiveness": 2,
                "rss": 3,
                "uss": 4
            },
            "data": data
        })
    }
}

#[derive(Debug)]
struct Sample {
    timestamp: f64,
    stack_index: Option<usize>,
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
