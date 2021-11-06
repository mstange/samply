pub use debugid;

use debugid::{CodeId, DebugId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use std::cmp::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod markers;

pub use markers::*;

#[derive(Debug)]
pub struct ProfileBuilder {
    pid: u32,
    interval: Duration,
    libs: Vec<Lib>,
    threads: HashMap<u32, ThreadBuilder>,
    start_time: Instant,
    start_time_system: SystemTime,
    end_time: Option<Instant>,
    command_name: String,
    subprocesses: Vec<ProfileBuilder>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub struct StringIndex(u32);

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub enum Frame {
    /// An instruction pointer address / return address
    Address(u64),
    /// A string, containing an index returned by ThreadBuilder::intern_string
    Label(StringIndex),
}

impl ProfileBuilder {
    pub fn new(
        start_time: Instant,
        start_time_system: SystemTime,
        command_name: &str,
        pid: u32,
        interval: Duration,
    ) -> Self {
        ProfileBuilder {
            pid,
            interval,
            threads: HashMap::new(),
            libs: Vec::new(),
            start_time,
            start_time_system,
            end_time: None,
            command_name: command_name.to_owned(),
            subprocesses: Vec::new(),
        }
    }

    pub fn set_start_time(&mut self, start_time: Instant) {
        self.start_time = start_time;
    }

    pub fn set_end_time(&mut self, end_time: Instant) {
        self.end_time = Some(end_time);
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_lib(
        &mut self,
        path: &Path,
        code_id: Option<CodeId>,
        debug_path: &Path,
        debug_id: DebugId,
        arch: Option<&str>,
        base_address: u64,
        address_range: std::ops::Range<u64>,
    ) {
        self.libs.push(Lib {
            path: path.to_owned(),
            debug_path: debug_path.to_owned(),
            arch: arch.map(|arch| arch.to_owned()),
            debug_id,
            code_id,
            base_address,
            start_address: address_range.start,
            end_address: address_range.end,
        })
    }

    pub fn add_thread(&mut self, thread_builder: ThreadBuilder) {
        self.threads.insert(thread_builder.index, thread_builder);
    }

    pub fn add_subprocess(&mut self, profile_builder: ProfileBuilder) {
        self.subprocesses.push(profile_builder);
    }

    fn collect_marker_schemas(&self) -> HashMap<&'static str, MarkerSchema> {
        let mut marker_schemas = HashMap::new();
        for thread in self.threads.values() {
            marker_schemas.extend(thread.marker_schemas.clone().into_iter());
        }
        for process in &self.subprocesses {
            marker_schemas.extend(process.collect_marker_schemas().into_iter());
        }
        marker_schemas
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut marker_schemas: Vec<MarkerSchema> =
            self.collect_marker_schemas().into_values().collect();
        marker_schemas.sort_by_key(|schema| schema.type_name);

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

        let start_time_ms_since_unix_epoch = self
            .start_time_system
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;

        let end_time_ms_since_start = self
            .end_time
            .map(|end_time| to_profile_timestamp(end_time, self.start_time));

        json!({
            "meta": {
                "version": 24,
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
                        "subcategories": ["Other"],
                    },
                    {
                        "name": "Other",
                        "color": "grey",
                        "subcategories": ["Other"],
                    }
                ],
                "sampleUnits": {
                    "time": "ms",
                    "eventDelay": "ms",
                    "threadCPUDelta": "µs"
                },
                "markerSchema": marker_schemas
            },
            "libs": libs,
            "threads": threads,
            "processes": subprocesses,
        })
    }
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
    markers: MarkerTable,
    marker_schemas: HashMap<&'static str, MarkerSchema>,
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
            markers: MarkerTable::new(),
            marker_schemas: HashMap::new(),
            string_table: StringTable::new(),
        }
    }

    pub fn set_start_time(&mut self, start_time: Instant) {
        self.start_time = start_time;
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

    pub fn intern_string(&mut self, s: &str) -> StringIndex {
        self.string_table.index_for_string(s)
    }

    pub fn add_sample(
        &mut self,
        timestamp: Instant,
        frames: impl Iterator<Item = Frame>,
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

    /// Main marker API to add a new marker to profiler buffer.
    pub fn add_marker<T: ProfilerMarker>(&mut self, name: &str, marker: T, timing: MarkerTiming) {
        self.marker_schemas
            .entry(T::MARKER_TYPE_NAME)
            .or_insert_with(T::schema);
        let name_string_index = self.string_table.index_for_string(name);
        self.markers.0.push(Marker {
            name_string_index,
            timing,
            data: marker.json_marker_data(),
        })
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        self.end_time = Some(end_time);
    }

    fn stack_index_for_frames(&mut self, frames: impl Iterator<Item = Frame>) -> Option<usize> {
        let frame_indexes: Vec<_> = frames
            .map(|frame| self.frame_index_for_frame(frame))
            .collect();
        self.stack_table.index_for_frames(&frame_indexes)
    }

    fn frame_index_for_frame(&mut self, frame: Frame) -> usize {
        self.frame_table
            .index_for_frame(&mut self.string_table, frame)
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
            "markers": self.markers.to_json(process_start),
            "stringTable": self.string_table.to_json()
        })
    }
}

#[derive(Debug)]
struct Lib {
    path: PathBuf,
    debug_path: PathBuf,
    arch: Option<String>,
    debug_id: DebugId,
    code_id: Option<CodeId>,
    base_address: u64,
    start_address: u64,
    end_address: u64,
}

impl Lib {
    pub fn to_json(&self) -> Value {
        let name = self.path.file_name().map(|f| f.to_string_lossy());
        let debug_name = self.debug_path.file_name().map(|f| f.to_string_lossy());
        let breakpad_id = format!("{}", self.debug_id.breakpad());
        let code_id = self.code_id.as_ref().map(|cid| cid.to_string());
        json!({
            "name": name,
            "path": self.path.to_string_lossy(),
            "debugName": debug_name,
            "debugPath": self.debug_path.to_string_lossy(),
            "breakpadId": breakpad_id,
            "codeId": code_id,
            "offset": self.start_address - self.base_address,
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
    frames: Vec<StringIndex>,

    // address -> frame index
    index: BTreeMap<Frame, usize>,
}

impl FrameTable {
    pub fn new() -> FrameTable {
        FrameTable {
            frames: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    pub fn index_for_frame(&mut self, string_table: &mut StringTable, frame: Frame) -> usize {
        let frames = &mut self.frames;
        *self.index.entry(frame.clone()).or_insert_with(|| {
            let frame_index = frames.len();
            let location_string_index = match frame {
                Frame::Address(address) => {
                    let location_string = format!("0x{:x}", address);
                    string_table.index_for_string(&location_string)
                }
                Frame::Label(string_index) => string_index,
            };
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
                let subcategory = 0;
                json!([
                    location.0,
                    false,
                    0,
                    null,
                    null,
                    null,
                    null,
                    category,
                    subcategory
                ])
            })
            .collect();
        json!({
            "schema": {
                "location": 0,
                "relevantForJS": 1,
                "innerWindowID": 2,
                "implementation": 3,
                "optimizations": 4,
                "line": 5,
                "column": 6,
                "category": 7,
                "subcategory": 8,
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
struct MarkerTable(Vec<Marker>);

impl MarkerTable {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn to_json(&self, process_start: Instant) -> Value {
        let data: Vec<Value> = self
            .0
            .iter()
            .map(|marker| marker.to_json(process_start))
            .collect();
        json!({
            "schema": {
                "name": 0,
                "startTime": 1,
                "endTime": 2,
                "phase": 3,
                "category": 4,
                "data": 5,
            },
            "data": data
        })
    }
}

#[repr(u8)]
enum Phase {
    Instant = 0,
    Interval = 1,
    IntervalStart = 2,
    IntervalEnd = 3,
}

#[derive(Debug, Clone)]
struct Marker {
    name_string_index: StringIndex,
    timing: MarkerTiming,
    data: Value,
}

impl Marker {
    fn to_json(&self, process_start: Instant) -> Value {
        let (s, e, phase) = match self.timing {
            MarkerTiming::Instant(s) => {
                (to_profile_timestamp(s, process_start), 0.0, Phase::Instant)
            }
            MarkerTiming::Interval(s, e) => (
                to_profile_timestamp(s, process_start),
                to_profile_timestamp(e, process_start),
                Phase::Interval,
            ),
            MarkerTiming::IntervalStart(s) => (
                to_profile_timestamp(s, process_start),
                0.0,
                Phase::IntervalStart,
            ),
            MarkerTiming::IntervalEnd(e) => (
                0.0,
                to_profile_timestamp(e, process_start),
                Phase::IntervalEnd,
            ),
        };
        json!([self.name_string_index.0, s, e, phase as u8, 0, self.data])
    }
}

#[derive(Debug)]
struct StringTable {
    strings: Vec<String>,
    index: HashMap<String, StringIndex>,
}

impl StringTable {
    pub fn new() -> Self {
        StringTable {
            strings: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn index_for_string(&mut self, s: &str) -> StringIndex {
        match self.index.get(s) {
            Some(string_index) => *string_index,
            None => {
                let string_index = StringIndex(self.strings.len() as u32);
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

#[cfg(test)]
mod test {
    use assert_json_diff::assert_json_eq;
    use serde_json::json;
    use std::time::{Duration, Instant, SystemTime};

    use crate::{
        MarkerDynamicField, MarkerFieldFormat, MarkerLocation, MarkerSchema, MarkerSchemaField,
        MarkerStaticField, MarkerTiming, ProfileBuilder, ProfilerMarker, TextMarker, ThreadBuilder,
    };

    #[test]
    fn it_works() {
        struct CustomMarker {
            event_name: String,
            allocation_size: u32,
            url: String,
            latency: Duration,
        }
        impl ProfilerMarker for CustomMarker {
            const MARKER_TYPE_NAME: &'static str = "custom";

            fn schema() -> MarkerSchema {
                MarkerSchema {
                    type_name: Self::MARKER_TYPE_NAME,
                    locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
                    chart_label: None,
                    tooltip_label: Some("Custom tooltip label"),
                    table_label: None,
                    fields: vec![
                        MarkerSchemaField::Dynamic(MarkerDynamicField {
                            key: "eventName",
                            label: "Event name",
                            format: MarkerFieldFormat::String,
                            searchable: None,
                        }),
                        MarkerSchemaField::Dynamic(MarkerDynamicField {
                            key: "allocationSize",
                            label: "Allocation size",
                            format: MarkerFieldFormat::Bytes,
                            searchable: None,
                        }),
                        MarkerSchemaField::Dynamic(MarkerDynamicField {
                            key: "url",
                            label: "URL",
                            format: MarkerFieldFormat::Url,
                            searchable: None,
                        }),
                        MarkerSchemaField::Dynamic(MarkerDynamicField {
                            key: "latency",
                            label: "Latency",
                            format: MarkerFieldFormat::Duration,
                            searchable: None,
                        }),
                        MarkerSchemaField::Static(MarkerStaticField {
                            label: "Description",
                            value: "This is a test marker with a custom schema.",
                        }),
                    ],
                }
            }

            fn json_marker_data(&self) -> serde_json::Value {
                json!({
                    "type": Self::MARKER_TYPE_NAME,
                    "eventName": self.event_name,
                    "allocationSize": self.allocation_size,
                    "url": self.url,
                    "latency": self.latency.as_secs_f64() * 1000.0,
                })
            }
        }

        let start_time = Instant::now();
        let start_time_system = SystemTime::UNIX_EPOCH + Duration::from_millis(1636162232627);
        let mut thread = ThreadBuilder::new(123, 12345, start_time, true, false);
        thread.add_sample(start_time, vec![].into_iter(), Duration::ZERO);
        thread.add_sample(
            start_time + Duration::from_millis(1),
            vec![].into_iter(),
            Duration::ZERO,
        );
        thread.add_sample(
            start_time + Duration::from_millis(2),
            vec![].into_iter(),
            Duration::ZERO,
        );
        thread.add_sample(
            start_time + Duration::from_millis(3),
            vec![].into_iter(),
            Duration::ZERO,
        );
        thread.add_marker(
            "Experimental",
            TextMarker("Hello world!".to_string()),
            MarkerTiming::Instant(start_time),
        );
        thread.add_marker(
            "CustomName",
            CustomMarker {
                event_name: "My event".to_string(),
                allocation_size: 512000,
                url: "https://mozilla.org/".to_string(),
                latency: Duration::from_millis(123),
            },
            MarkerTiming::Interval(start_time, start_time + Duration::from_millis(2)),
        );
        let mut profile = ProfileBuilder::new(
            start_time,
            start_time_system,
            "test",
            123,
            Duration::from_millis(1),
        );
        profile.add_thread(thread);
        let result = profile.to_json();
        // println!("{}", result.to_string());
        assert_json_eq!(
            result,
            json!(
                {
                    "libs": [],
                    "meta": {
                      "categories": [
                        { "color": "blue", "name": "Regular", "subcategories": ["Other"] },
                        { "color": "grey", "name": "Other", "subcategories": ["Other"] }
                      ],
                      "interval": 1.0,
                      "markerSchema": [
                        {
                          "chartLabel": "{marker.data.name}",
                          "data": [{ "format": "string", "key": "name", "label": "Details" }],
                          "display": ["marker-chart", "marker-table"],
                          "name": "Text",
                          "tableLabel": "{marker.name} - {marker.data.name}"
                        },
                        {
                          "data": [
                            { "format": "string", "key": "eventName", "label": "Event name" },
                            {
                              "format": "bytes",
                              "key": "allocationSize",
                              "label": "Allocation size"
                            },
                            { "format": "url", "key": "url", "label": "URL" },
                            { "format": "duration", "key": "latency", "label": "Latency" },
                            {
                              "label": "Description",
                              "value": "This is a test marker with a custom schema."
                            }
                          ],
                          "display": ["marker-chart", "marker-table"],
                          "name": "custom",
                          "tooltipLabel": "Custom tooltip label"
                        }
                      ],
                      "pausedRanges": [],
                      "pid": 123,
                      "processType": 0,
                      "product": "test",
                      "sampleUnits": { "eventDelay": "ms", "threadCPUDelta": "µs", "time": "ms" },
                      "shutdownTime": null,
                      "startTime": 1636162232627.0,
                      "version": 24
                    },
                    "processes": [],
                    "threads": [
                      {
                        "frameTable": {
                          "data": [],
                          "schema": {
                            "category": 7,
                            "column": 6,
                            "implementation": 3,
                            "innerWindowID": 2,
                            "line": 5,
                            "location": 0,
                            "optimizations": 4,
                            "relevantForJS": 1,
                            "subcategory": 8
                          }
                        },
                        "markers": {
                          "data": [
                            [0, 0.0, 0.0, 0, 0, { "name": "Hello world!", "type": "Text" }],
                            [
                              1,
                              0.0,
                              2.0,
                              1,
                              0,
                              {
                                "allocationSize": 512000,
                                "eventName": "My event",
                                "latency": 123.0,
                                "type": "custom",
                                "url": "https://mozilla.org/"
                              }
                            ]
                          ],
                          "schema": {
                            "category": 4,
                            "data": 5,
                            "endTime": 2,
                            "name": 0,
                            "phase": 3,
                            "startTime": 1
                          }
                        },
                        "name": "GeckoMain",
                        "pid": 123,
                        "processName": "test",
                        "processType": "default",
                        "registerTime": 0.0,
                        "samples": {
                          "data": [
                            [null, 0.0, 0.0, 0],
                            [null, 1.0, 0.0, 0],
                            [null, 2.0, 0.0, 0],
                            [null, 3.0, 0.0, 0]
                          ],
                          "schema": {
                            "eventDelay": 2,
                            "stack": 0,
                            "threadCPUDelta": 3,
                            "time": 1
                          }
                        },
                        "stackTable": { "data": [], "schema": { "frame": 1, "prefix": 0 } },
                        "stringTable": ["Experimental", "CustomName"],
                        "tid": 12345,
                        "unregisterTime": null
                      }
                    ]
                  }
            )
        )
    }
}
