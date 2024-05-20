use fxprof_processed_profile::{
    CategoryHandle, CategoryPairHandle, LibMappings, MarkerDynamicField, MarkerFieldFormat,
    MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerStaticField, MarkerTiming, Profile,
    ProfilerMarker, ThreadHandle, Timestamp,
};
use serde_json::json;

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
        let stack_converter = StackConverter::new(user_category, kernel_category);
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
            profile.add_marker(
                marker.thread_handle,
                CategoryHandle::OTHER,
                "SimpleMarker",
                SimpleMarker(marker.name.clone()),
                MarkerTiming::Interval(marker.start_time, marker.end_time),
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct RssStatMarker(pub i64, pub i64);

impl ProfilerMarker for RssStatMarker {
    const MARKER_TYPE_NAME: &'static str = "RSS Anon";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "totalBytes": self.0,
            "deltaBytes": self.1
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.totalBytes}"),
            tooltip_label: Some("{marker.data.totalBytes}"),
            table_label: Some("Total: {marker.data.totalBytes}, delta: {marker.data.deltaBytes}"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "totalBytes",
                    label: "Total bytes",
                    format: MarkerFieldFormat::Bytes,
                    searchable: true,
                }),
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "deltaBytes",
                    label: "Delta",
                    format: MarkerFieldFormat::Bytes,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Emitted when the kmem:rss_stat tracepoint is hit.",
                }),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OtherEventMarker;

impl ProfilerMarker for OtherEventMarker {
    const MARKER_TYPE_NAME: &'static str = "Other event";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![MarkerSchemaField::Static(MarkerStaticField {
                label: "Description",
                value:
                    "Emitted for any records in a perf.data file which don't map to a known event.",
            })],
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserTimingMarker(pub String);

impl ProfilerMarker for UserTimingMarker {
    const MARKER_TYPE_NAME: &'static str = "UserTiming";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "name": self.0,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}"),
            tooltip_label: Some("{marker.data.name}"),
            table_label: Some("{marker.data.name}"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "name",
                    label: "Name",
                    format: MarkerFieldFormat::String,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Emitted for performance.mark and performance.measure.",
                }),
            ],
        }
    }
}

pub struct SchedSwitchMarkerOnCpuTrack;

impl ProfilerMarker for SchedSwitchMarkerOnCpuTrack {
    const MARKER_TYPE_NAME: &'static str = "sched_switch";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![MarkerSchemaField::Static(MarkerStaticField {
                label: "Description",
                value: "Emitted just before a running thread gets moved off-cpu.",
            })],
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedSwitchMarkerOnThreadTrack {
    pub cpu: u32,
}

impl ProfilerMarker for SchedSwitchMarkerOnThreadTrack {
    const MARKER_TYPE_NAME: &'static str = "sched_switch";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "cpu": self.cpu,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: None,
            tooltip_label: None,
            table_label: None,
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "cpu",
                    label: "cpu",
                    format: MarkerFieldFormat::Integer,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Emitted just before a running thread gets moved off-cpu.",
                }),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimpleMarker(pub String);

impl ProfilerMarker for SimpleMarker {
    const MARKER_TYPE_NAME: &'static str = "SimpleMarker";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "name": self.0,
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}"),
            tooltip_label: Some("{marker.data.name}"),
            table_label: Some("{marker.data.name}"),
            fields: vec![
                MarkerSchemaField::Dynamic(MarkerDynamicField {
                    key: "name",
                    label: "Name",
                    format: MarkerFieldFormat::String,
                    searchable: true,
                }),
                MarkerSchemaField::Static(MarkerStaticField {
                    label: "Description",
                    value: "Emitted for marker spans in a markers text file.",
                }),
            ],
        }
    }
}
