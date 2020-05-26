use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug)]
pub struct ProfileBuilder {
    libs: Vec<Lib>,
    threads: HashMap<u32, ThreadBuilder>,
}

impl ProfileBuilder {
    pub fn new() -> Self {
        ProfileBuilder {
            threads: HashMap::new(),
            libs: Vec::new(),
        }
    }

    pub fn add_lib(&mut self, name: &str, path: &str, uuid: &Uuid, address_range: &std::ops::Range<u64>) {
        self.libs.push(Lib {
            name: name.to_string(),
            path: path.to_string(),
            breakpad_id: format!("{:X}0", uuid.to_simple()),
            start_address: address_range.start,
            end_address: address_range.end,
        })
    }

    pub fn add_sample(&mut self, thread_index: u32, timestamp: f64, frames: &[u64]) {
        let thread = self
            .threads
            .entry(thread_index)
            .or_insert_with(|| ThreadBuilder::new(thread_index));
        thread.add_sample(timestamp, frames);
    }

    pub fn to_json(&self) -> serde_json::Value {
        let threads: Vec<Value> = self
            .threads
            .iter()
            .map(|(_, thread)| thread.to_json())
            .collect();
        let libs: Vec<Value> = self.libs.iter().map(|l| l.to_json()).collect();
        json!({
            "meta": {
                "version": 4,
                "processType": 0,
                "interval": 1
            },
            "libs": libs,
            "threads": threads,
        })
    }
}

#[derive(Debug)]
pub struct ThreadBuilder {
    index: u32,
    stack_table: StackTable,
    frame_table: FrameTable,
    samples: SampleTable,
    string_table: StringTable,
}

impl ThreadBuilder {
    pub fn new(thread_index: u32) -> Self {
        ThreadBuilder {
            index: thread_index,
            stack_table: StackTable::new(),
            frame_table: FrameTable::new(),
            samples: SampleTable(Vec::new()),
            string_table: StringTable::new(),
        }
    }

    pub fn add_sample(&mut self, timestamp: f64, frames: &[u64]) {
        let stack_index = self.stack_index_for_frames(frames);
        self.samples.0.push(Sample {
            timestamp,
            stack_index,
        });
    }

    fn stack_index_for_frames(&mut self, frames: &[u64]) -> Option<usize> {
        let frame_indexes: Vec<_> = frames
            .iter()
            .map(|&address| self.frame_index_for_address(address))
            .collect();
        self.stack_table.index_for_frames(&frame_indexes)
    }

    fn frame_index_for_address(&mut self, address: u64) -> usize {
        let location_string = format!("0x{:x}", address);
        let location_string_index = self.string_table.index_for_string(&location_string);
        self.frame_table.index_for_location(location_string_index)
    }

    fn to_json(&self) -> Value {
        json!({
            "name": "All",
            "processType": "default",
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
    frame_locations: Vec<usize>,
    index: HashMap<usize, usize>,
}

impl FrameTable {
    pub fn new() -> FrameTable {
        FrameTable {
            frame_locations: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn index_for_location(&mut self, location_string_index: usize) -> usize {
        match self.index.get(&location_string_index) {
            Some(frame_index) => *frame_index,
            None => {
                let frame_index = self.frame_locations.len();
                self.frame_locations.push(location_string_index);
                self.index.insert(location_string_index, frame_index);
                frame_index
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let data: Vec<Value> = self.frame_locations.iter().map(|l| json!([l])).collect();
        json!({
            "schema": {
                "location": 0,
                "implementation": 1,
                "optimizations": 2,
                "line": 3,
                "category": 4
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
