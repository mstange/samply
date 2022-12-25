use std::cmp::Ordering;

use serde::ser::SerializeMap;
use serde::Serializer;
use serde_json::json;

use crate::category::{Category, CategoryPairHandle};
use crate::cpu_delta::CpuDelta;
use crate::frame_and_func_table::{
    FrameTableAndFuncTable, InternalFrame, ThreadInternalStringIndex,
};
use crate::global_lib_table::GlobalLibTable;
use crate::marker_table::MarkerTable;
use crate::resource_table::ResourceTable;
use crate::sample_table::SampleTable;
use crate::stack_table::StackTable;
use crate::string_table::{GlobalStringIndex, GlobalStringTable};
use crate::thread_string_table::ThreadStringTable;
use crate::{MarkerTiming, ProfilerMarker, Timestamp};

/// A process. Can be created with [`Profile::add_process`](crate::Profile::add_process).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ProcessHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Thread {
    process: ProcessHandle,
    tid: u32,
    name: Option<String>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    is_main: bool,
    stack_table: StackTable,
    frame_table_and_func_table: FrameTableAndFuncTable,
    samples: SampleTable,
    markers: MarkerTable,
    resources: ResourceTable,
    string_table: ThreadStringTable,
    last_sample_stack: Option<usize>,
    last_sample_was_zero_cpu: bool,
}

impl Thread {
    pub fn new(process: ProcessHandle, tid: u32, start_time: Timestamp, is_main: bool) -> Self {
        Self {
            process,
            tid,
            name: None,
            start_time,
            end_time: None,
            is_main,
            stack_table: StackTable::new(),
            frame_table_and_func_table: FrameTableAndFuncTable::new(),
            samples: SampleTable::new(),
            markers: MarkerTable::new(),
            resources: ResourceTable::new(),
            string_table: ThreadStringTable::new(),
            last_sample_stack: None,
            last_sample_was_zero_cpu: false,
        }
    }
    pub fn set_name(&mut self, name: &str) {
        self.name = Some(name.to_string());
    }

    pub fn set_start_time(&mut self, start_time: Timestamp) {
        self.start_time = start_time;
    }

    pub fn set_end_time(&mut self, end_time: Timestamp) {
        self.end_time = Some(end_time);
    }

    pub fn process(&self) -> ProcessHandle {
        self.process
    }

    pub fn convert_string_index(
        &mut self,
        global_table: &GlobalStringTable,
        index: GlobalStringIndex,
    ) -> ThreadInternalStringIndex {
        self.string_table
            .index_for_global_string(index, global_table)
    }

    pub fn frame_index_for_frame(
        &mut self,
        frame: InternalFrame,
        global_libs: &GlobalLibTable,
    ) -> usize {
        self.frame_table_and_func_table.index_for_frame(
            &mut self.string_table,
            &mut self.resources,
            global_libs,
            frame,
        )
    }

    pub fn stack_index_for_stack(
        &mut self,
        prefix: Option<usize>,
        frame: usize,
        category_pair: CategoryPairHandle,
    ) -> usize {
        self.stack_table
            .index_for_stack(prefix, frame, category_pair)
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        self.samples
            .add_sample(timestamp, stack_index, cpu_delta, weight);
        self.last_sample_stack = stack_index;
        self.last_sample_was_zero_cpu = cpu_delta == CpuDelta::ZERO;
    }

    pub fn add_sample_same_stack_zero_cpu(&mut self, timestamp: Timestamp, weight: i32) {
        if self.last_sample_was_zero_cpu {
            self.samples.modify_last_sample(timestamp, weight);
        } else {
            let stack_index = self.last_sample_stack;
            self.samples
                .add_sample(timestamp, stack_index, CpuDelta::ZERO, weight);
            self.last_sample_was_zero_cpu = true;
        }
    }

    pub fn add_marker<T: ProfilerMarker>(&mut self, name: &str, marker: T, timing: MarkerTiming) {
        let name_string_index = self.string_table.index_for_string(name);
        self.markers
            .add_marker(name_string_index, timing, marker.json_marker_data());
    }

    pub fn cmp_for_json_order(&self, other: &Thread) -> Ordering {
        if let Some(ordering) = self.start_time.partial_cmp(&other.start_time) {
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        let ordering = self.name.cmp(&other.name);
        if ordering != Ordering::Equal {
            return ordering;
        }
        self.tid.cmp(&other.tid)
    }

    pub fn serialize_with<S: Serializer>(
        &self,
        serializer: S,
        categories: &[Category],
        process_start_time: Timestamp,
        process_end_time: Option<Timestamp>,
        process_name: &str,
        pid: u32,
    ) -> Result<S::Ok, S::Error> {
        let thread_name = if self.is_main {
            // https://github.com/firefox-devtools/profiler/issues/2508
            "GeckoMain".to_string()
        } else if let Some(name) = &self.name {
            name.clone()
        } else {
            format!("Thread <{}>", self.tid)
        };

        let thread_register_time = self.start_time;
        let thread_unregister_time = self.end_time;

        let native_symbols = json!({
            "length": 0,
            "address": [],
            "libIndex": [],
            "name": [],
        });

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(
            "frameTable",
            &self.frame_table_and_func_table.as_frame_table(categories),
        )?;
        map.serialize_entry(
            "funcTable",
            &self.frame_table_and_func_table.as_func_table(),
        )?;
        map.serialize_entry("markers", &self.markers)?;
        map.serialize_entry("name", &thread_name)?;
        map.serialize_entry("nativeSymbols", &native_symbols)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("pid", &pid)?;
        map.serialize_entry("processName", process_name)?;
        map.serialize_entry("processShutdownTime", &process_end_time)?;
        map.serialize_entry("processStartupTime", &process_start_time)?;
        map.serialize_entry("processType", &"default")?;
        map.serialize_entry("registerTime", &thread_register_time)?;
        map.serialize_entry("resourceTable", &self.resources)?;
        map.serialize_entry("samples", &self.samples)?;
        map.serialize_entry(
            "stackTable",
            &self.stack_table.serialize_with_categories(categories),
        )?;
        map.serialize_entry("stringArray", &self.string_table)?;
        map.serialize_entry("tid", &self.tid)?;
        map.serialize_entry("unregisterTime", &thread_unregister_time)?;
        map.end()
    }
}
