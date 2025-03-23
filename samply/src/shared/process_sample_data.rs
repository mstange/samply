use fxprof_processed_profile::{
    LibMappings, MarkerFieldFlags, MarkerFieldFormat, MarkerTiming, Profile, StaticSchemaMarker,
    StaticSchemaMarkerField, StringHandle, SubcategoryHandle, ThreadHandle, Timestamp,
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
        user_category: SubcategoryHandle,
        kernel_category: SubcategoryHandle,
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
                thread_handle,
                stack_frame_scratch_buf,
                &lib_mappings_hierarchy,
                extra_label_frame,
            );
            let mut frames =
                StackDepthLimitingFrameIter::new(profile, frames, thread_handle, user_category);
            let stack_handle =
                profile.handle_for_stack_frames(thread_handle, move |p| frames.next(p));
            match sample_or_marker {
                SampleOrMarker::Sample(SampleData { cpu_delta, weight }) => {
                    profile.add_sample(thread_handle, timestamp, stack_handle, cpu_delta, weight);
                }
                SampleOrMarker::MarkerHandle(mh) => {
                    profile.set_marker_stack(thread_handle, mh, stack_handle);
                }
            }
        }

        for marker in marker_spans {
            let marker_name_string_index = profile.handle_for_string(&marker.name);
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

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.totalBytes}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.totalBytes}");
    const TABLE_LABEL: Option<&'static str> =
        Some("Total: {marker.data.totalBytes}, delta: {marker.data.deltaBytes}");

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted when the kmem:rss_stat tracepoint is hit.");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[
        StaticSchemaMarkerField {
            key: "totalBytes",
            label: "Total bytes",
            format: MarkerFieldFormat::Bytes,
            flags: MarkerFieldFlags::SEARCHABLE,
        },
        StaticSchemaMarkerField {
            key: "deltaBytes",
            label: "Delta",
            format: MarkerFieldFormat::Bytes,
            flags: MarkerFieldFlags::SEARCHABLE,
        },
    ];

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.name
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

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted for any records in a perf.data file which don't map to a known event.");

    const FIELDS: &'static [fxprof_processed_profile::StaticSchemaMarkerField] = &[];

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.0
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

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted for performance.mark and performance.measure.");

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.data.name}");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "name",
        label: "Name",
        format: MarkerFieldFormat::String,
        flags: MarkerFieldFlags::SEARCHABLE,
    }];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("UserTiming")
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

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted just before a running thread gets moved off-cpu.");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("sched_switch")
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

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted just before a running thread gets moved off-cpu.");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "cpu",
        label: "cpu",
        format: MarkerFieldFormat::Integer,
        flags: MarkerFieldFlags::SEARCHABLE,
    }];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("sched_switch")
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

    const DESCRIPTION: Option<&'static str> =
        Some("Emitted for marker spans in a markers text file.");

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.data.name}");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "name",
        label: "Name",
        format: MarkerFieldFormat::String,
        flags: MarkerFieldFlags::SEARCHABLE,
    }];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("SimpleMarker")
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.0
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}
