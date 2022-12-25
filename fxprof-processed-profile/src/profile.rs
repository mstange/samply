use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::json;

use crate::category::{Category, CategoryHandle, CategoryPairHandle};
use crate::category_color::CategoryColor;
use crate::cpu_delta::CpuDelta;
use crate::fast_hash_map::FastHashMap;
use crate::frame::Frame;
use crate::frame_and_func_table::{InternalFrame, InternalFrameLocation};
use crate::global_lib_table::GlobalLibTable;
use crate::library_info::LibraryInfo;
use crate::process::{Process, ThreadHandle};
use crate::reference_timestamp::ReferenceTimestamp;
use crate::string_table::{GlobalStringIndex, GlobalStringTable};
use crate::thread::{ProcessHandle, Thread};
use crate::{MarkerSchema, MarkerTiming, ProfilerMarker, Timestamp};

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StringHandle(GlobalStringIndex);

#[derive(Debug)]
pub struct Profile {
    pub(crate) product: String,
    pub(crate) interval: Duration,
    pub(crate) libs: GlobalLibTable,
    pub(crate) categories: Vec<Category>, // append-only for stable CategoryHandles
    pub(crate) processes: Vec<Process>,   // append-only for stable ProcessHandles
    pub(crate) threads: Vec<Thread>,      // append-only for stable ThreadHandles
    pub(crate) reference_timestamp: ReferenceTimestamp,
    pub(crate) string_table: GlobalStringTable,
    pub(crate) marker_schemas: FastHashMap<&'static str, MarkerSchema>,
}

impl Profile {
    pub fn new(product: &str, reference_timestamp: ReferenceTimestamp, interval: Duration) -> Self {
        Profile {
            interval,
            product: product.to_string(),
            threads: Vec::new(),
            libs: GlobalLibTable::new(),
            reference_timestamp,
            processes: Vec::new(),
            string_table: GlobalStringTable::new(),
            marker_schemas: FastHashMap::default(),
            categories: vec![Category {
                name: "Other".to_string(),
                color: CategoryColor::Grey,
                subcategories: Vec::new(),
            }],
        }
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    pub fn set_reference_timestamp(&mut self, reference_timestamp: ReferenceTimestamp) {
        self.reference_timestamp = reference_timestamp;
    }

    pub fn set_product(&mut self, product: &str) {
        self.product = product.to_string();
    }

    pub fn add_category(&mut self, name: &str, color: CategoryColor) -> CategoryHandle {
        let handle = CategoryHandle(self.categories.len() as u16);
        self.categories.push(Category {
            name: name.to_string(),
            color,
            subcategories: Vec::new(),
        });
        handle
    }

    pub fn add_subcategory(&mut self, category: CategoryHandle, name: &str) -> CategoryPairHandle {
        let subcategory = self.categories[category.0 as usize].add_subcategory(name.into());
        CategoryPairHandle(category, Some(subcategory))
    }

    pub fn add_process(&mut self, name: &str, pid: u32, start_time: Timestamp) -> ProcessHandle {
        let handle = ProcessHandle(self.processes.len());
        self.processes.push(Process::new(name, pid, start_time));
        handle
    }

    pub fn set_process_start_time(&mut self, process: ProcessHandle, start_time: Timestamp) {
        self.processes[process.0].set_start_time(start_time);
    }

    pub fn set_process_end_time(&mut self, process: ProcessHandle, end_time: Timestamp) {
        self.processes[process.0].set_end_time(end_time);
    }

    pub fn set_process_name(&mut self, process: ProcessHandle, name: &str) {
        self.processes[process.0].set_name(name);
    }

    pub fn add_lib(&mut self, process: ProcessHandle, library: LibraryInfo) {
        self.processes[process.0].add_lib(library);
    }

    pub fn unload_lib(&mut self, process: ProcessHandle, base_address: u64) {
        self.processes[process.0].unload_lib(base_address);
    }

    pub fn add_thread(
        &mut self,
        process: ProcessHandle,
        tid: u32,
        start_time: Timestamp,
        is_main: bool,
    ) -> ThreadHandle {
        let handle = ThreadHandle(self.threads.len());
        self.threads
            .push(Thread::new(process, tid, start_time, is_main));
        self.processes[process.0].add_thread(handle);
        handle
    }

    pub fn set_thread_name(&mut self, thread: ThreadHandle, name: &str) {
        self.threads[thread.0].set_name(name);
    }

    pub fn set_thread_start_time(&mut self, thread: ThreadHandle, start_time: Timestamp) {
        self.threads[thread.0].set_start_time(start_time);
    }

    pub fn set_thread_end_time(&mut self, thread: ThreadHandle, end_time: Timestamp) {
        self.threads[thread.0].set_end_time(end_time);
    }

    pub fn intern_string(&mut self, s: &str) -> StringHandle {
        StringHandle(self.string_table.index_for_string(s))
    }

    pub fn add_sample(
        &mut self,
        thread: ThreadHandle,
        timestamp: Timestamp,
        frames: impl Iterator<Item = (Frame, CategoryPairHandle)>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        let stack_index = self.stack_index_for_frames(thread, frames);
        self.threads[thread.0].add_sample(timestamp, stack_index, cpu_delta, weight);
    }

    pub fn add_sample_same_stack_zero_cpu(
        &mut self,
        thread: ThreadHandle,
        timestamp: Timestamp,
        weight: i32,
    ) {
        self.threads[thread.0].add_sample_same_stack_zero_cpu(timestamp, weight);
    }

    pub fn add_marker<T: ProfilerMarker>(
        &mut self,
        thread: ThreadHandle,
        name: &str,
        marker: T,
        timing: MarkerTiming,
    ) {
        self.marker_schemas
            .entry(T::MARKER_TYPE_NAME)
            .or_insert_with(T::schema);
        self.threads[thread.0].add_marker(name, marker, timing);
    }

    // frames is ordered from caller to callee, i.e. root function first, pc last
    fn stack_index_for_frames(
        &mut self,
        thread: ThreadHandle,
        frames: impl Iterator<Item = (Frame, CategoryPairHandle)>,
    ) -> Option<usize> {
        let thread = &mut self.threads[thread.0];
        let process = &mut self.processes[thread.process().0];
        let mut prefix = None;
        for (frame, category_pair) in frames {
            let location = match frame {
                Frame::InstructionPointer(ip) => process.convert_address(&mut self.libs, ip),
                Frame::ReturnAddress(ra) => {
                    process.convert_address(&mut self.libs, ra.saturating_sub(1))
                }
                Frame::Label(string_index) => {
                    let thread_string_index =
                        thread.convert_string_index(&self.string_table, string_index.0);
                    InternalFrameLocation::Label(thread_string_index)
                }
            };
            let internal_frame = InternalFrame {
                location,
                category_pair,
            };
            let frame_index = thread.frame_index_for_frame(internal_frame, &self.libs);
            prefix = Some(thread.stack_index_for_stack(prefix, frame_index, category_pair));
        }
        prefix
    }
}

impl Serialize for Profile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("meta", &SerializableProfileMeta(self))?;
        map.serialize_entry("libs", &self.libs)?;
        map.serialize_entry("threads", &SerializableProfileThreadsProperty(self))?;
        map.serialize_entry("pages", &[] as &[()])?;
        map.serialize_entry("profilerOverhead", &[] as &[()])?;
        map.serialize_entry("counters", &[] as &[()])?;
        map.end()
    }
}

struct SerializableProfileMeta<'a>(&'a Profile);

impl<'a> Serialize for SerializableProfileMeta<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("categories", &self.0.categories)?;
        map.serialize_entry("debug", &false)?;
        map.serialize_entry(
            "extensions",
            &json!({
                "length": 0,
                "baseURL": [],
                "id": [],
                "name": [],
            }),
        )?;
        map.serialize_entry("interval", &(self.0.interval.as_secs_f64() * 1000.0))?;
        map.serialize_entry("preprocessedProfileVersion", &41)?;
        map.serialize_entry("processType", &0)?;
        map.serialize_entry("product", &self.0.product)?;
        map.serialize_entry(
            "sampleUnits",
            &json!({
                "time": "ms",
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
            }),
        )?;
        map.serialize_entry("startTime", &self.0.reference_timestamp)?;
        map.serialize_entry("symbolicated", &false)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("version", &24)?;

        let mut marker_schemas: Vec<MarkerSchema> =
            self.0.marker_schemas.values().cloned().collect();
        marker_schemas.sort_by_key(|schema| schema.type_name);
        map.serialize_entry("markerSchema", &marker_schemas)?;

        map.end()
    }
}

struct SerializableProfileThreadsProperty<'a>(&'a Profile);

impl<'a> Serialize for SerializableProfileThreadsProperty<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The processed profile format has all threads from all processes in a flattened threads list.
        // Each thread duplicates some information about its process, which allows the Firefox Profiler
        // UI to group threads from the same process.

        let mut seq = serializer.serialize_seq(Some(self.0.threads.len()))?;

        let mut sorted_processes: Vec<_> = (0..self.0.processes.len()).map(ProcessHandle).collect();
        sorted_processes.sort_by(|a_handle, b_handle| {
            let a = &self.0.processes[a_handle.0];
            let b = &self.0.processes[b_handle.0];
            a.cmp_for_json_order(b)
        });

        for process in sorted_processes {
            let mut sorted_threads = self.0.processes[process.0].threads();
            sorted_threads.sort_by(|a_handle, b_handle| {
                let a = &self.0.threads[a_handle.0];
                let b = &self.0.threads[b_handle.0];
                a.cmp_for_json_order(b)
            });

            for thread in sorted_threads {
                let categories = &self.0.categories;
                let thread = &self.0.threads[thread.0];
                let process = &self.0.processes[thread.process().0];
                seq.serialize_element(&SerializableProfileThread(process, thread, categories))?;
            }
        }

        seq.end()
    }
}

struct SerializableProfileThread<'a>(&'a Process, &'a Thread, &'a [Category]);

impl<'a> Serialize for SerializableProfileThread<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SerializableProfileThread(process, thread, categories) = self;
        let process_start_time = process.start_time();
        let process_end_time = process.end_time();
        let process_name = process.name();
        let pid = process.pid();
        thread.serialize_with(
            serializer,
            categories,
            process_start_time,
            process_end_time,
            process_name,
            pid,
        )
    }
}
