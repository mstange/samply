use fxprof_processed_profile::{
    CategoryPairHandle, LibMappings, MarkerDynamicField, MarkerFieldFormat, MarkerLocation,
    MarkerSchema, MarkerSchemaField, MarkerStaticField, MarkerTiming, Profile, ProfilerMarker,
    ThreadHandle, Timestamp,
};
use rangemap::RangeSet;
use serde_json::json;

use super::{
    lib_mappings::{LibMappingInfo, LibMappingOpQueue, LibMappingsHierarchy},
    marker_file::MarkerSpan,
    stack_converter::StackConverter,
    stack_depth_limiting_frame_iter::StackDepthLimitingFrameIter,
    types::StackFrame,
    unresolved_samples::{
        OtherEventMarkerData, RssStatMarkerData, SampleData, SampleOrMarker,
        UnresolvedSampleOrMarker, UnresolvedSamples, UnresolvedStacks,
    },
};

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
    main_thread_handle: ThreadHandle,
}

impl ProcessSampleData {
    pub fn new(
        unresolved_samples: UnresolvedSamples,
        regular_lib_mapping_op_queue: LibMappingOpQueue,
        jitdump_lib_mapping_op_queues: Vec<LibMappingOpQueue>,
        perf_map_mappings: Option<LibMappings<LibMappingInfo>>,
        main_thread_handle: ThreadHandle,
    ) -> Self {
        Self {
            unresolved_samples,
            regular_lib_mapping_op_queue,
            jitdump_lib_mapping_op_queues,
            perf_map_mappings,
            main_thread_handle,
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
        event_names: &[String],
        marker_spans: &[MarkerSpan],
        sample_range_set: Option<&RangeSet<Timestamp>>,
    ) {
        let ProcessSampleData {
            unresolved_samples,
            regular_lib_mapping_op_queue,
            jitdump_lib_mapping_op_queues,
            perf_map_mappings,
            main_thread_handle,
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

            if sample_range_set.is_some()
                && !sample_range_set.as_ref().unwrap().contains(&timestamp)
            {
                continue;
            }

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
                SampleOrMarker::RssStatMarker(RssStatMarkerData {
                    size,
                    delta,
                    member,
                }) => {
                    let timing = MarkerTiming::Instant(timestamp);
                    let name = match member {
                        RssStatMember::ResidentFileMappingPages => "RSS Stat FILEPAGES",
                        RssStatMember::ResidentAnonymousPages => "RSS Stat ANONPAGES",
                        RssStatMember::AnonymousSwapEntries => "RSS Stat SHMEMPAGES",
                        RssStatMember::ResidentSharedMemoryPages => "RSS Stat SWAPENTS",
                    };
                    profile.add_marker_with_stack(
                        thread_handle,
                        name,
                        RssStatMarker(size, delta),
                        timing,
                        frames,
                    );
                }
                SampleOrMarker::OtherEventMarker(OtherEventMarkerData { attr_index }) => {
                    if let Some(name) = event_names.get(attr_index) {
                        let timing = MarkerTiming::Instant(timestamp);
                        profile.add_marker_with_stack(
                            thread_handle,
                            name,
                            OtherEventMarker,
                            timing,
                            frames,
                        );
                    }
                }
            }
        }

        for marker in marker_spans {
            profile.add_marker(
                main_thread_handle,
                "UserTiming",
                UserTimingMarker(marker.name.clone()),
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
                    value: "Emitted for marker spans in a markers text file.",
                }),
            ],
        }
    }
}
