use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use debugid;
use debugid::{CodeId, DebugId};
use serde_json::{json, Value};

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

    pub fn to_serializable(&self) -> SerializableProfile {
        SerializableProfile(self)
    }
}

pub struct SerializableProfile<'a>(&'a ProfileBuilder);

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

impl<'a> Serialize for SerializableProfile<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let start_time_ms_since_unix_epoch = self
            .0
            .start_time_system
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;

        let end_time_ms_since_start = self
            .0
            .end_time
            .map(|end_time| to_profile_timestamp(end_time, self.0.start_time));

        let mut marker_schemas: Vec<MarkerSchema> =
            self.0.collect_marker_schemas().into_values().collect();
        marker_schemas.sort_by_key(|schema| schema.type_name);

        let meta = json!({
            "version": 24,
            "startTime": start_time_ms_since_unix_epoch,
            "shutdownTime": end_time_ms_since_start,
            "pausedRanges": [],
            "product": self.0.command_name,
            "interval": self.0.interval.as_secs_f64() * 1000.0,
            "pid": self.0.pid,
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
        });

        let mut sorted_threads: Vec<_> = self.0.threads.values().collect();
        sorted_threads.sort_by(|a, b| {
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

        let command_name = self.0.command_name.clone();
        let threads: Vec<_> = sorted_threads
            .into_iter()
            .map(|thread| thread.to_serializable(&command_name, self.0.start_time))
            .collect();

        let mut libs: Vec<_> = self.0.libs.iter().collect();
        libs.sort_by_key(|l| l.start_address);

        let mut sorted_subprocesses: Vec<_> = self.0.subprocesses.iter().collect();
        sorted_subprocesses.sort_by(|a, b| {
            if let Some(ordering) = a.start_time.partial_cmp(&b.start_time) {
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            a.pid.cmp(&b.pid)
        });

        let subprocesses: Vec<_> = sorted_subprocesses
            .iter()
            .map(|p| p.to_serializable())
            .collect();

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("meta", &meta)?;
        map.serialize_entry("libs", &libs)?;
        map.serialize_entry("threads", &threads)?;
        map.serialize_entry("processes", &subprocesses)?;
        map.end()
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

    fn to_serializable<'a, 'n>(
        &'a self,
        process_name: &'n str,
        process_start: Instant,
    ) -> SerializableProfileThread<'a, 'n> {
        SerializableProfileThread {
            thread: self,
            process_name,
            process_start,
        }
    }
}

pub struct SerializableProfileThread<'a, 'n> {
    thread: &'a ThreadBuilder,
    process_name: &'n str,
    process_start: Instant,
}

impl<'a, 'n> Serialize for SerializableProfileThread<'a, 'n> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = if self.thread.is_main {
            // https://github.com/firefox-devtools/profiler/issues/2508
            "GeckoMain".to_string()
        } else if let Some(name) = &self.thread.name {
            name.clone()
        } else if self.thread.is_libdispatch_thread {
            "libdispatch".to_string()
        } else {
            format!("Thread <{}>", self.thread.index)
        };
        let register_time = to_profile_timestamp(self.thread.start_time, self.process_start);
        let unregister_time = self
            .thread
            .end_time
            .map(|end_time| to_profile_timestamp(end_time, self.process_start));

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &name)?;
        map.serialize_entry("tid", &self.thread.index)?;
        map.serialize_entry("pid", &self.thread.pid)?;
        map.serialize_entry("processType", &"default")?;
        map.serialize_entry("processName", &self.process_name)?;
        map.serialize_entry("registerTime", &register_time)?;
        map.serialize_entry("unregisterTime", &unregister_time)?;
        map.serialize_entry("frameTable", &self.thread.frame_table)?;
        map.serialize_entry("stackTable", &self.thread.stack_table)?;
        map.serialize_entry(
            "samples",
            &self.thread.samples.to_serializable(self.process_start),
        )?;
        map.serialize_entry(
            "markers",
            &self.thread.markers.to_serializable(self.process_start),
        )?;
        map.serialize_entry("stringTable", &self.thread.string_table.to_serializable())?;
        map.end()
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

impl Serialize for Lib {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = self.path.file_name().map(|f| f.to_string_lossy());
        let debug_name = self.debug_path.file_name().map(|f| f.to_string_lossy());
        let breakpad_id = format!("{}", self.debug_id.breakpad());
        let code_id = self.code_id.as_ref().map(|cid| cid.to_string());
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &name)?;
        map.serialize_entry("path", &self.path.to_string_lossy())?;
        map.serialize_entry("debugName", &debug_name)?;
        map.serialize_entry("debugPath", &self.debug_path.to_string_lossy())?;
        map.serialize_entry("breakpadId", &breakpad_id)?;
        map.serialize_entry("codeId", &code_id)?;
        map.serialize_entry("offset", &(self.start_address - self.base_address))?;
        map.serialize_entry("start", &self.start_address)?;
        map.serialize_entry("end", &self.end_address)?;
        map.serialize_entry("arch", &self.arch)?;
        map.end()
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
}

impl Serialize for StackTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = json!({
            "prefix": 0,
            "frame": 1,
        });

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schema", &schema)?;
        map.serialize_entry("data", &SerializableStackTableData(self))?;
        map.end()
    }
}

struct SerializableStackTableData<'a>(&'a StackTable);

impl<'a> Serialize for SerializableStackTableData<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.stacks.len()))?;
        for stack in &self.0.stacks {
            seq.serialize_element(&SerializableStackTableDataValue(stack))?;
        }
        seq.end()
    }
}

struct SerializableStackTableDataValue<'a>(&'a (Option<usize>, usize));

impl<'a> Serialize for SerializableStackTableDataValue<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (prefix, frame_index) = self.0;
        let mut seq = serializer.serialize_seq(Some(2))?;
        seq.serialize_element(prefix)?;
        seq.serialize_element(frame_index)?;
        seq.end()
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
                    let location_string = format!("0x{address:x}");
                    string_table.index_for_string(&location_string)
                }
                Frame::Label(string_index) => string_index,
            };
            frames.push(location_string_index);
            frame_index
        })
    }
}

impl Serialize for FrameTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = json!({
            "location": 0,
            "relevantForJS": 1,
            "innerWindowID": 2,
            "implementation": 3,
            "optimizations": 4,
            "line": 5,
            "column": 6,
            "category": 7,
            "subcategory": 8,
        });
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schema", &schema)?;
        map.serialize_entry("data", &SerializableFrameTableData(self))?;
        map.end()
    }
}

struct SerializableFrameTableData<'a>(&'a FrameTable);

impl<'a> Serialize for SerializableFrameTableData<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.frames.len()))?;
        for location in &self.0.frames {
            seq.serialize_element(&SerializableFrameTableDataValue(*location))?;
        }
        seq.end()
    }
}

struct SerializableFrameTableDataValue(StringIndex);

impl Serialize for SerializableFrameTableDataValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(9))?;
        seq.serialize_element(&self.0 .0)?; // location
        seq.serialize_element(&false)?; // relevantForJS
        seq.serialize_element(&0)?; // innerWindowID
        seq.serialize_element(&())?; // implementation
        seq.serialize_element(&())?; // optimizations
        seq.serialize_element(&())?; // line
        seq.serialize_element(&())?; // column
        seq.serialize_element(&0)?; // category
        seq.serialize_element(&0)?; // subcategory
        seq.end()
    }
}

#[derive(Debug)]
struct SampleTable(Vec<Sample>);

impl SampleTable {
    fn to_serializable(&self, process_start: Instant) -> SerializableSampleTable<'_> {
        SerializableSampleTable {
            table: self,
            process_start,
        }
    }
}

struct SerializableSampleTable<'a> {
    table: &'a SampleTable,
    process_start: Instant,
}

impl<'a> Serialize for SerializableSampleTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = json!({
            "stack": 0,
            "time": 1,
            "eventDelay": 2,
            "threadCPUDelta": 3
        });
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schema", &schema)?;
        map.serialize_entry(
            "data",
            &SerializableSampleTableData {
                table: self.table,
                process_start: self.process_start,
            },
        )?;
        map.end()
    }
}
struct SerializableSampleTableData<'a> {
    table: &'a SampleTable,
    process_start: Instant,
}

impl<'a> Serialize for SerializableSampleTableData<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.table.0.len()))?;
        for sample in &self.table.0 {
            seq.serialize_element(&SerializableSampleTableDataValue {
                stack_index: sample.stack_index,
                timestamp: to_profile_timestamp(sample.timestamp, self.process_start),
                cpu_delta_us: sample.cpu_delta_us,
            })?;
        }
        seq.end()
    }
}

struct SerializableSampleTableDataValue {
    stack_index: Option<usize>,
    timestamp: f64,
    cpu_delta_us: u64,
}

impl Serialize for SerializableSampleTableDataValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(4))?;
        seq.serialize_element(&self.stack_index)?;
        seq.serialize_element(&self.timestamp)?;
        seq.serialize_element(&0.0)?;
        seq.serialize_element(&self.cpu_delta_us)?;
        seq.end()
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

    fn to_serializable(&self, process_start: Instant) -> SerializableMarkerTable<'_> {
        SerializableMarkerTable {
            table: self,
            process_start,
        }
    }
}

struct SerializableMarkerTable<'a> {
    table: &'a MarkerTable,
    process_start: Instant,
}

impl<'a> Serialize for SerializableMarkerTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = json!({
            "name": 0,
            "startTime": 1,
            "endTime": 2,
            "phase": 3,
            "category": 4,
            "data": 5,
        });
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schema", &schema)?;
        map.serialize_entry(
            "data",
            &SerializableMarkerTableData {
                table: self.table,
                process_start: self.process_start,
            },
        )?;
        map.end()
    }
}
struct SerializableMarkerTableData<'a> {
    table: &'a MarkerTable,
    process_start: Instant,
}

impl<'a> Serialize for SerializableMarkerTableData<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.table.0.len()))?;
        for marker in &self.table.0 {
            seq.serialize_element(&marker.to_serializable(self.process_start))?;
        }
        seq.end()
    }
}

struct SerializableMarkerTableDataValue<'a> {
    name_string_index: StringIndex,
    start: f64,
    end: f64,
    phase: u8,
    data: &'a Value,
}

impl<'a> Serialize for SerializableMarkerTableDataValue<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(6))?;
        seq.serialize_element(&self.name_string_index.0)?; // name
        seq.serialize_element(&self.start)?; // startTime
        seq.serialize_element(&self.end)?; // endTime
        seq.serialize_element(&self.phase)?; // phase
        seq.serialize_element(&0)?; // category
        seq.serialize_element(self.data)?; // data
        seq.end()
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
    fn to_serializable(&self, process_start: Instant) -> SerializableMarkerTableDataValue<'_> {
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
        SerializableMarkerTableDataValue {
            name_string_index: self.name_string_index,
            start: s,
            end: e,
            phase: phase as u8,
            data: &self.data,
        }
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

    fn to_serializable(&self) -> &[String] {
        &self.strings
    }
}

#[cfg(test)]
mod test {
    use std::time::{Duration, Instant, SystemTime};

    use assert_json_diff::assert_json_eq;
    use serde_json::json;

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
        let result = profile.to_serializable();
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
