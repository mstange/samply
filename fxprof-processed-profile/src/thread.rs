use std::borrow::Cow;
use std::cmp::Ordering;

use serde::ser::{SerializeMap, Serializer};

use crate::cpu_delta::CpuDelta;
use crate::marker_table::MarkerTable;
use crate::markers::InternalMarkerSchema;
use crate::sample_table::{NativeAllocationsTable, SampleTable, WeightType};
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::{
    DynamicSchemaMarker, MarkerHandle, MarkerTiming, MarkerTypeHandle, StackHandle, Timestamp,
};

/// A process. Can be created with [`Profile::add_process`](crate::Profile::add_process).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ProcessHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Thread {
    process: ProcessHandle,
    tid: String,
    name: Option<String>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    is_main: bool,
    samples: SampleTable,
    native_allocations: Option<NativeAllocationsTable>,
    markers: MarkerTable,
    last_sample_stack: Option<StackHandle>,
    last_sample_was_zero_cpu: bool,
    show_markers_in_timeline: bool,
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
            samples: SampleTable::new(),
            native_allocations: None,
            markers: MarkerTable::new(),
            last_sample_stack: None,
            last_sample_was_zero_cpu: false,
            show_markers_in_timeline: false,
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

    pub fn set_tid(&mut self, tid: String) {
        self.tid = tid;
    }

    pub fn set_show_markers_in_timeline(&mut self, v: bool) {
        self.show_markers_in_timeline = v;
    }

    pub fn process(&self) -> ProcessHandle {
        self.process
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<StackHandle>,
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
        stack_index: Option<StackHandle>,
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

    pub fn set_samples_weight_type(&mut self, t: WeightType) {
        self.samples.set_weight_type(t);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_marker<T: DynamicSchemaMarker>(
        &mut self,
        string_table: &mut ProfileStringTable,
        name_string_index: StringHandle,
        marker_type_handle: MarkerTypeHandle,
        schema: &InternalMarkerSchema,
        marker: T,
        timing: MarkerTiming,
    ) -> MarkerHandle {
        self.markers.add_marker(
            string_table,
            name_string_index,
            marker_type_handle,
            schema,
            marker,
            timing,
        )
    }

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<StackHandle>) {
        self.markers.set_marker_stack(marker, stack_index);
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

    pub fn make_symbolicated_thread(
        self,
        old_stack_to_new_stack: &[Option<StackHandle>],
    ) -> Thread {
        let Thread {
            process,
            tid,
            name,
            start_time,
            end_time,
            is_main,
            samples,
            native_allocations,
            markers,
            last_sample_stack,
            last_sample_was_zero_cpu,
            show_markers_in_timeline,
        } = self;

        let samples = samples.with_remapped_stacks(old_stack_to_new_stack);
        let native_allocations = native_allocations.map(|native_allocations| {
            native_allocations.with_remapped_stacks(old_stack_to_new_stack)
        });
        let markers = markers.with_remapped_stacks(old_stack_to_new_stack);
        let last_sample_stack = last_sample_stack
            .and_then(|last_sample_stack| old_stack_to_new_stack[last_sample_stack.0]);

        Thread {
            process,
            tid,
            name,
            start_time,
            end_time,
            is_main,
            samples,
            native_allocations,
            markers,
            last_sample_stack,
            last_sample_was_zero_cpu,
            show_markers_in_timeline,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn serialize_with<S: Serializer>(
        &self,
        serializer: S,
        process_start_time: Timestamp,
        process_end_time: Option<Timestamp>,
        process_name: &str,
        pid: &str,
        marker_schemas: &[InternalMarkerSchema],
        string_table: &ProfileStringTable,
    ) -> Result<S::Ok, S::Error> {
        let thread_name: Cow<str> = match (self.is_main, &self.name) {
            (true, _) => process_name.into(),
            (false, Some(name)) => name.into(),
            (false, None) => format!("Thread <{}>", self.tid).into(),
        };

        let thread_register_time = self.start_time;
        let thread_unregister_time = self.end_time;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &thread_name)?;
        map.serialize_entry("isMainThread", &self.is_main)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("pid", &pid)?;
        map.serialize_entry("processName", process_name)?;
        map.serialize_entry("processShutdownTime", &process_end_time)?;
        map.serialize_entry("processStartupTime", &process_start_time)?;
        map.serialize_entry("processType", &"default")?;
        map.serialize_entry("registerTime", &thread_register_time)?;
        map.serialize_entry("tid", &self.tid)?;
        map.serialize_entry("unregisterTime", &thread_unregister_time)?;
        map.serialize_entry("showMarkersInTimeline", &self.show_markers_in_timeline)?;
        map.serialize_entry("samples", &self.samples)?;
        if let Some(allocations) = &self.native_allocations {
            map.serialize_entry("nativeAllocations", &allocations)?;
        }
        map.serialize_entry(
            "markers",
            &self.markers.as_serializable(marker_schemas, string_table),
        )?;
        map.end()
    }
}
