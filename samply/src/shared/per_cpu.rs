use fxprof_processed_profile::{
    CategoryHandle, FrameFlags, FrameHandle, MarkerFieldFormat, MarkerTiming, ProcessHandle,
    Profile, StaticSchemaMarker, StaticSchemaMarkerField, StringHandle, ThreadHandle, Timestamp,
};

use crate::shared::context_switch::ThreadContextSwitchData;
use crate::shared::timestamp_converter::TimestampConverter;

pub struct Cpus {
    start_time: Timestamp,
    process_handle: ProcessHandle,
    combined_thread_handle: ThreadHandle,
    cpus: Vec<Cpu>,
}

pub struct Cpu {
    pub name: StringHandle,
    pub thread_handle: ThreadHandle,
    pub context_switch_data: ThreadContextSwitchData,
    pub idle_frame: FrameHandle,
    current_tid: Option<(i32, StringHandle, u64)>,
}

impl Cpu {
    pub fn new(name: StringHandle, thread_handle: ThreadHandle, idle_frame: FrameHandle) -> Self {
        Self {
            name,
            thread_handle,
            context_switch_data: Default::default(),
            idle_frame,
            current_tid: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify_switch_in_for_marker(
        &mut self,
        tid: i32,
        thread_name: StringHandle,
        timestamp: u64,
        converter: &TimestampConverter,
        thread_handles: &[ThreadHandle],
        profile: &mut Profile,
    ) {
        let previous_tid = self.current_tid.replace((tid, thread_name, timestamp));
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
    pub fn notify_switch_out_for_marker(
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
                true => profile.handle_for_string("preempted"),
                false => profile.handle_for_string("blocked"),
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
        Self {
            start_time,
            process_handle,
            combined_thread_handle,
            cpus: Vec::new(),
        }
    }

    pub fn combined_thread_handle(&self) -> ThreadHandle {
        self.combined_thread_handle
    }

    pub fn get_mut(&mut self, cpu: usize, profile: &mut Profile) -> &mut Cpu {
        while self.cpus.len() <= cpu {
            let i = self.cpus.len();
            let thread = profile.add_thread(self.process_handle, i as u32, self.start_time, false);
            let name = format!("CPU {i}");
            profile.set_thread_name(thread, &name);
            let idle_string = profile.handle_for_string("<Idle>");
            let idle_frame = profile.handle_for_frame_with_label(
                thread,
                idle_string,
                CategoryHandle::OTHER,
                FrameFlags::empty(),
            );
            self.cpus.push(Cpu::new(
                profile.handle_for_string(&name),
                thread,
                idle_frame,
            ));
        }
        &mut self.cpus[cpu]
    }
}

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct ThreadNameMarkerForCpuTrack(pub StringHandle, pub StringHandle);

impl StaticSchemaMarker for ThreadNameMarkerForCpuTrack {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "ContextSwitch";

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.thread}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.thread}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.thread}");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "thread",
        label: "Thread",
        format: MarkerFieldFormat::String,
    }];

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.0
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.1
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }

    fn flow_field_value(&self, _field_index: u32) -> u64 {
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

    const CHART_LABEL: Option<&'static str> = Some("{marker.data.cpu}");
    const TOOLTIP_LABEL: Option<&'static str> = Some("{marker.data.cpu}");
    const TABLE_LABEL: Option<&'static str> =
        Some("{marker.name} - {marker.data.cpu}, switch-out reason: {marker.data.outwhy}");

    const FIELDS: &'static [StaticSchemaMarkerField] = &[
        StaticSchemaMarkerField {
            key: "cpu",
            label: "CPU",
            format: MarkerFieldFormat::String,
        },
        StaticSchemaMarkerField {
            key: "outwhy",
            label: "Switch-out reason",
            format: MarkerFieldFormat::String,
        },
    ];

    fn name(&self, profile: &mut Profile) -> StringHandle {
        profile.handle_for_string("Running on CPU")
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

    fn flow_field_value(&self, _field_index: u32) -> u64 {
        unreachable!()
    }
}
