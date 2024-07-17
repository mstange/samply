use fxprof_processed_profile::{
    CategoryHandle, Frame, FrameFlags, FrameInfo, MarkerFieldFormat, MarkerFieldSchema,
    MarkerLocation, MarkerSchema, MarkerTiming, ProcessHandle, Profile, StaticSchemaMarker,
    StringHandle, ThreadHandle, Timestamp,
};

use crate::shared::context_switch::ThreadContextSwitchData;
use crate::shared::timestamp_converter::TimestampConverter;

pub struct Cpus {
    start_time: Timestamp,
    process_handle: ProcessHandle,
    combined_thread_handle: ThreadHandle,
    cpus: Vec<Cpu>,
    idle_frame_label: FrameInfo,
}

pub struct Cpu {
    pub name: StringHandle,
    pub thread_handle: ThreadHandle,
    pub context_switch_data: ThreadContextSwitchData,
    pub current_tid: Option<(i32, StringHandle, u64)>,
}

impl Cpu {
    pub fn new(name: StringHandle, thread_handle: ThreadHandle) -> Self {
        Self {
            name,
            thread_handle,
            context_switch_data: Default::default(),
            current_tid: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify_switch_in(
        &mut self,
        tid: i32,
        thread_name: StringHandle,
        timestamp: u64,
        converter: &TimestampConverter,
        thread_handles: &[ThreadHandle],
        profile: &mut Profile,
    ) {
        let previous_tid =
            std::mem::replace(&mut self.current_tid, Some((tid, thread_name, timestamp)));
        if let Some((_previous_tid, previous_thread_name, switch_in_timestamp)) = previous_tid {
            // eprintln!("Missing switch-out (noticed during switch-in) on {}: {previous_tid}, {switch_in_timestamp}", profile.get_string(self.name));
            let start_timestamp = converter.convert_time(switch_in_timestamp);
            let end_timestamp = converter.convert_time(timestamp);
            let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
            for thread_handle in thread_handles {
                profile.add_marker(
                    *thread_handle,
                    timing.clone(),
                    ThreadNameMarkerForCpuTrack(self.name, previous_thread_name),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify_switch_out(
        &mut self,
        tid: i32, // tid that is being switched away from
        timestamp: u64,
        converter: &TimestampConverter,
        thread_handles: &[ThreadHandle], // for cpu tracks
        thread_handle: ThreadHandle,     // for thread tracks
        preempted: bool,
        profile: &mut Profile,
    ) {
        let previous_tid = self.current_tid.take();
        if let Some((previous_tid, previous_thread_name, switch_in_timestamp)) = previous_tid {
            let start_timestamp = converter.convert_time(switch_in_timestamp);
            let end_timestamp = converter.convert_time(timestamp);
            let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
            for thread_handle in thread_handles {
                profile.add_marker(
                    *thread_handle,
                    timing.clone(),
                    ThreadNameMarkerForCpuTrack(self.name, previous_thread_name),
                );
            }
            let switch_out_reason = match preempted {
                true => profile.intern_string("preempted"),
                false => profile.intern_string("blocked"),
            };
            profile.add_marker(
                thread_handle,
                timing.clone(),
                OnCpuMarkerForThreadTrack {
                    cpu_name: self.name,
                    switch_out_reason,
                },
            );
            if previous_tid != tid {
                // eprintln!("Missing switch-out (noticed during switch-out) on {}: {previous_tid}, {switch_in_timestamp}", profile.get_string(self.name));
                // eprintln!(
                //     "Missing switch-in (noticed during switch-out) on {}: {tid}, {timestamp}",
                //     profile.get_string(self.name)
                // );
            }
        } else {
            // eprintln!(
            //     "Missing switch-in (noticed during switch-out) on {}: {tid}, {timestamp}",
            //     profile.get_string(self.name)
            // );
        }
    }
}

impl Cpus {
    pub fn new(start_time: Timestamp, profile: &mut Profile) -> Self {
        let process_handle = profile.add_process("CPU", 0, start_time);
        let combined_thread_handle = profile.add_thread(process_handle, 0, start_time, true);
        let idle_string = profile.intern_string("<Idle>");
        let idle_frame_label = FrameInfo {
            frame: Frame::Label(idle_string),
            category_pair: CategoryHandle::OTHER.into(),
            flags: FrameFlags::empty(),
        };
        Self {
            start_time,
            process_handle,
            combined_thread_handle,
            cpus: Vec::new(),
            idle_frame_label,
        }
    }

    pub fn combined_thread_handle(&self) -> ThreadHandle {
        self.combined_thread_handle
    }

    pub fn idle_frame_label(&self) -> FrameInfo {
        self.idle_frame_label.clone()
    }

    pub fn get_mut(&mut self, cpu: usize, profile: &mut Profile) -> &mut Cpu {
        while self.cpus.len() <= cpu {
            let i = self.cpus.len();
            let thread = profile.add_thread(self.process_handle, i as u32, self.start_time, false);
            let name = format!("CPU {i}");
            profile.set_thread_name(thread, &name);
            self.cpus
                .push(Cpu::new(profile.intern_string(&name), thread));
        }
        &mut self.cpus[cpu]
    }
}

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct ThreadNameMarkerForCpuTrack(pub StringHandle, pub StringHandle);

impl StaticSchemaMarker for ThreadNameMarkerForCpuTrack {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "ContextSwitch";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.thread}".into()),
            tooltip_label: Some("{marker.data.thread}".into()),
            table_label: Some("{marker.name} - {marker.data.thread}".into()),
            fields: vec![MarkerFieldSchema {
                key: "thread".into(),
                label: "Thread".into(),
                format: MarkerFieldFormat::String,
                searchable: true,
            }],
            static_fields: vec![],
        }
    }

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.0
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.1
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct OnCpuMarkerForThreadTrack {
    cpu_name: StringHandle,
    switch_out_reason: StringHandle,
}

impl StaticSchemaMarker for OnCpuMarkerForThreadTrack {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "OnCpu";

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.cpu}".into()),
            tooltip_label: Some("{marker.data.cpu}".into()),
            table_label: Some(
                "{marker.name} - {marker.data.cpu}, switch-out reason: {marker.data.outwhy}".into(),
            ),
            fields: vec![
                MarkerFieldSchema {
                    key: "cpu".into(),
                    label: "CPU".into(),
                    format: MarkerFieldFormat::String,
                    searchable: true,
                },
                MarkerFieldSchema {
                    key: "outwhy".into(),
                    label: "Switch-out reason".into(),
                    format: MarkerFieldFormat::String,
                    searchable: true,
                },
            ],
            static_fields: vec![],
        }
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.intern_string("Running on CPU")
    }

    fn category(&self, _profile: &mut Profile) -> CategoryHandle {
        CategoryHandle::OTHER
    }

    fn string_field_value(&self, field_index: u32) -> StringHandle {
        match field_index {
            0 => self.cpu_name,
            1 => self.switch_out_reason,
            _ => unreachable!(),
        }
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }
}
