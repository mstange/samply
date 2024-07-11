use fxprof_processed_profile::{
    CategoryHandle, CategoryPairHandle, LibMappings, MarkerFieldFormat, MarkerFieldSchema,
    MarkerLocation, MarkerSchema, MarkerStaticField, MarkerTiming, Profile, StaticSchemaMarker,
    StringHandle, ThreadHandle, Timestamp,
};

use super::lib_mappings::{LibMappingInfo, LibMappingOpQueue, LibMappingsHierarchy};
use super::stack_converter::StackConverter;
use super::stack_depth_limiting_frame_iter::StackDepthLimitingFrameIter;
use super::types::StackFrame;
use super::unresolved_samples::{
    SampleData, SampleOrMarker, UnresolvedSampleOrMarker, UnresolvedSamples, UnresolvedStacks,
};

#[derive(Debug, Clone)]
pub struct MarkerSpanOnThread {
    pub thread_handle: ThreadHandle,
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub name: String,
}

#[derive(Debug, Clone)]
pub enum RssStatMember {
    ResidentFileMappingPages,
    ResidentAnonymousPages,
    AnonymousSwapEntries,
    ResidentSharedMemoryPages,
}

#[derive(Debug, Clone)]
pub struct ProcessSampleData {
    unresolved_samples: UnresolvedSamples,
    regular_lib_mapping_op_queue: LibMappingOpQueue,
    jitdump_lib_mapping_op_queues: Vec<LibMappingOpQueue>,
    perf_map_mappings: Option<LibMappings<LibMappingInfo>>,
    marker_spans: Vec<MarkerSpanOnThread>,
}

impl ProcessSampleData {
    pub fn new(
        unresolved_samples: UnresolvedSamples,
        regular_lib_mapping_op_queue: LibMappingOpQueue,
        jitdump_lib_mapping_op_queues: Vec<LibMappingOpQueue>,
        perf_map_mappings: Option<LibMappings<LibMappingInfo>>,
        marker_spans: Vec<MarkerSpanOnThread>,
    ) -> Self {
        Self {
            unresolved_samples,
            regular_lib_mapping_op_queue,
            jitdump_lib_mapping_op_queues,
            perf_map_mappings,
            marker_spans,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.unresolved_samples.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn flush_samples_to_profile(
        self,
        profile: &mut Profile,
        user_category: CategoryPairHandle,
        kernel_category: CategoryPairHandle,
        stack_frame_scratch_buf: &mut Vec<StackFrame>,
        stacks: &UnresolvedStacks,
    ) {
        let ProcessSampleData {
            unresolved_samples,
            regular_lib_mapping_op_queue,
            jitdump_lib_mapping_op_queues,
            perf_map_mappings,
            marker_spans,
        } = self;
        let mut lib_mappings_hierarchy = LibMappingsHierarchy::new(regular_lib_mapping_op_queue);
        for jitdump_lib_mapping_ops in jitdump_lib_mapping_op_queues {
            lib_mappings_hierarchy.add_jitdump_lib_mappings_ops(jitdump_lib_mapping_ops);
        }
        if let Some(perf_map_mappings) = perf_map_mappings {
            lib_mappings_hierarchy.add_perf_map_mappings(perf_map_mappings);
        }
        let mut stack_converter = StackConverter::new(user_category, kernel_category);
        let samples = unresolved_samples.into_inner();
        for sample in samples {
            lib_mappings_hierarchy.process_ops(sample.timestamp_mono);
            let UnresolvedSampleOrMarker {
                thread_handle,
                timestamp,
                stack,
                sample_or_marker,
                extra_label_frame,
                ..
            } = sample;

            stack_frame_scratch_buf.clear();
            stacks.convert_back(stack, stack_frame_scratch_buf);
            let frames = stack_converter.convert_stack(
                stack_frame_scratch_buf,
                &lib_mappings_hierarchy,
                extra_label_frame,
            );
            let frames = StackDepthLimitingFrameIter::new(profile, frames, user_category);
            match sample_or_marker {
                SampleOrMarker::Sample(SampleData { cpu_delta, weight }) => {
                    profile.add_sample(thread_handle, timestamp, frames, cpu_delta, weight);
                }
                SampleOrMarker::MarkerHandle(mh) => {
                    profile.set_marker_stack(thread_handle, mh, frames);
                }
            }
        }

        for marker in marker_spans {
            let marker_name_string_index = profile.intern_string(&marker.name);
            profile.add_marker(
                marker.thread_handle,
                MarkerTiming::Interval(marker.start_time, marker.end_time),
                SimpleMarker(marker_name_string_index),
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct RssStatMarker {
    pub name: StringHandle,
    pub total_bytes: i64,
    pub delta_bytes: i64,
}

impl RssStatMarker {
    pub fn new(name: StringHandle, total_bytes: i64, delta_bytes: i64) -> Self {
        Self {
            name,
            total_bytes,
            delta_bytes,
        }
    }
}

impl StaticSchemaMarker for RssStatMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "RSS Anon";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.totalBytes}".into()),
            tooltip_label: Some("{marker.data.totalBytes}".into()),
            table_label: Some(
                "Total: {marker.data.totalBytes}, delta: {marker.data.deltaBytes}".into(),
            ),
            fields: vec![
                MarkerFieldSchema {
                    key: "totalBytes".into(),
                    label: "Total bytes".into(),
                    format: MarkerFieldFormat::Bytes,
                    searchable: true,
                },
                MarkerFieldSchema {
                    key: "deltaBytes".into(),
                    label: "Delta".into(),
                    format: MarkerFieldFormat::Bytes,
                    searchable: true,
                },
            ],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted when the kmem:rss_stat tracepoint is hit.".into(),
            }],
        }
    }

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.name
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        unreachable!()
    }

    fn number_field_value(&self, field_index: u32) -> f64 {
        match field_index {
            0 => self.total_bytes as f64,
            1 => self.delta_bytes as f64,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OtherEventMarker(pub StringHandle);

impl StaticSchemaMarker for OtherEventMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "Other event";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value:
                    "Emitted for any records in a perf.data file which don't map to a known event."
                        .into(),
            }],
        }
    }

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.0
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        unreachable!()
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}

#[derive(Debug, Clone)]
pub struct UserTimingMarker(pub StringHandle);

impl StaticSchemaMarker for UserTimingMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "UserTiming";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}".into()),
            tooltip_label: Some("{marker.data.name}".into()),
            table_label: Some("{marker.data.name}".into()),
            fields: vec![MarkerFieldSchema {
                key: "name".into(),
                label: "Name".into(),
                format: MarkerFieldFormat::String,
                searchable: true,
            }],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted for performance.mark and performance.measure.".into(),
            }],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("UserTiming")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.0
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}

pub struct SchedSwitchMarkerOnCpuTrack;

impl StaticSchemaMarker for SchedSwitchMarkerOnCpuTrack {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "sched_switch";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted just before a running thread gets moved off-cpu.".into(),
            }],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("sched_switch")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        unreachable!()
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}

#[derive(Debug, Clone)]
pub struct SchedSwitchMarkerOnThreadTrack {
    pub cpu: u32,
}

impl StaticSchemaMarker for SchedSwitchMarkerOnThreadTrack {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "sched_switch";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![MarkerFieldSchema {
                key: "cpu".into(),
                label: "cpu".into(),
                format: MarkerFieldFormat::Integer,
                searchable: true,
            }],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted just before a running thread gets moved off-cpu.".into(),
            }],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("sched_switch")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        unreachable!()
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        self.cpu.into()
    }
}

#[derive(Debug, Clone)]
pub struct SimpleMarker(pub StringHandle);

impl StaticSchemaMarker for SimpleMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "SimpleMarker";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}".into()),
            tooltip_label: Some("{marker.data.name}".into()),
            table_label: Some("{marker.data.name}".into()),
            fields: vec![MarkerFieldSchema {
                key: "name".into(),
                label: "Name".into(),
                format: MarkerFieldFormat::String,
                searchable: true,
            }],
            static_fields: vec![MarkerStaticField {
                label: "Description".into(),
                value: "Emitted for marker spans in a markers text file.".into(),
            }],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("SimpleMarker")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.0
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}
