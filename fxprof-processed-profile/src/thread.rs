use std::borrow::Cow;
use std::cmp::Ordering;

use serde::ser::{SerializeMap, Serializer};

use crate::category::{Category, CategoryPairHandle};
use crate::cpu_delta::CpuDelta;
use crate::frame_table::{FrameTable, InternalFrame};
use crate::func_table::FuncTable;
use crate::global_lib_table::GlobalLibTable;
use crate::marker_table::MarkerTable;
use crate::markers::InternalMarkerSchema;
use crate::native_symbols::NativeSymbols;
use crate::resource_table::ResourceTable;
use crate::sample_table::{NativeAllocationsTable, SampleTable};
use crate::stack_table::StackTable;
use crate::string_table::{GlobalStringIndex, GlobalStringTable};
use crate::thread_string_table::{ThreadInternalStringIndex, ThreadStringTable};
use crate::{CategoryHandle, Marker, MarkerHandle, MarkerTiming, MarkerTypeHandle, Timestamp};

/// A process. Can be created with [`Profile::add_process`](crate::Profile::add_process).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ProcessHandle(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CounterHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Thread {
    process: ProcessHandle,
    tid: String,
    name: Option<String>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    is_main: bool,
    stack_table: StackTable,
    frame_table: FrameTable,
    func_table: FuncTable,
    samples: SampleTable,
    native_allocations: Option<NativeAllocationsTable>,
    markers: MarkerTable,
    resources: ResourceTable,
    native_symbols: NativeSymbols,
    string_table: ThreadStringTable,
    last_sample_stack: Option<usize>,
    last_sample_was_zero_cpu: bool,
}

impl Thread {
    pub fn new(process: ProcessHandle, tid: String, start_time: Timestamp, is_main: bool) -> Self {
        Self {
            process,
            tid,
            name: None,
            start_time,
            end_time: None,
            is_main,
            stack_table: StackTable::new(),
            frame_table: FrameTable::new(),
            func_table: FuncTable::new(),
            samples: SampleTable::new(),
            native_allocations: None,
            markers: MarkerTable::new(),
            resources: ResourceTable::new(),
            native_symbols: NativeSymbols::new(),
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
        global_libs: &mut GlobalLibTable,
    ) -> usize {
        self.frame_table.index_for_frame(
            &mut self.string_table,
            &mut self.resources,
            &mut self.func_table,
            &mut self.native_symbols,
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

    pub fn add_allocation_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        // Create allocations table, if it doesn't exist yet.
        let allocations = self.native_allocations.get_or_insert_with(Default::default);

        // Add the allocation sample.
        allocations.add_sample(timestamp, stack_index, allocation_address, allocation_size);
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

    #[allow(clippy::too_many_arguments)]
    pub fn add_marker<T: Marker>(
        &mut self,
        name_string_index: ThreadInternalStringIndex,
        marker_type_handle: MarkerTypeHandle,
        schema: &InternalMarkerSchema,
        marker: T,
        timing: MarkerTiming,
        category: CategoryHandle,
        global_string_table: &mut GlobalStringTable,
    ) -> MarkerHandle {
        self.markers.add_marker(
            name_string_index,
            marker_type_handle,
            schema,
            marker,
            timing,
            category,
            &mut self.string_table,
            global_string_table,
        )
    }

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<usize>) {
        self.markers.set_marker_stack(marker, stack_index);
    }

    pub fn contains_js_function(&self) -> bool {
        self.func_table.contains_js_function()
    }

    pub fn cmp_for_json_order(&self, other: &Thread) -> Ordering {
        let ordering = (!self.is_main).cmp(&(!other.is_main));
        if ordering != Ordering::Equal {
            return ordering;
        }
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

    #[allow(clippy::too_many_arguments)]
    pub fn serialize_with<S: Serializer>(
        &self,
        serializer: S,
        categories: &[Category],
        process_start_time: Timestamp,
        process_end_time: Option<Timestamp>,
        process_name: &str,
        pid: &str,
        marker_schemas: &[InternalMarkerSchema],
        global_string_table: &GlobalStringTable,
    ) -> Result<S::Ok, S::Error> {
        let thread_name: Cow<str> = match (self.is_main, &self.name) {
            (true, _) => process_name.into(),
            (false, Some(name)) => name.into(),
            (false, None) => format!("Thread <{}>", self.tid).into(),
        };

        let thread_register_time = self.start_time;
        let thread_unregister_time = self.end_time;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("frameTable", &self.frame_table.as_serializable(categories))?;
        map.serialize_entry("funcTable", &self.func_table)?;
        map.serialize_entry(
            "markers",
            &self
                .markers
                .as_serializable(marker_schemas, global_string_table),
        )?;
        map.serialize_entry("name", &thread_name)?;
        map.serialize_entry("isMainThread", &self.is_main)?;
        map.serialize_entry("nativeSymbols", &self.native_symbols)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("pid", &pid)?;
        map.serialize_entry("processName", process_name)?;
        map.serialize_entry("processShutdownTime", &process_end_time)?;
        map.serialize_entry("processStartupTime", &process_start_time)?;
        map.serialize_entry("processType", &"default")?;
        map.serialize_entry("registerTime", &thread_register_time)?;
        map.serialize_entry("resourceTable", &self.resources)?;
        map.serialize_entry("samples", &self.samples)?;
        if let Some(allocations) = &self.native_allocations {
            map.serialize_entry("nativeAllocations", &allocations)?;
        }
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
