use fxprof_processed_profile::{
    CategoryHandle, Frame, FrameFlags, FrameInfo, MarkerDynamicField, MarkerFieldFormat,
    MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerTiming, ProcessHandle, Profile,
    ProfilerMarker, StringHandle, ThreadHandle, Timestamp,
};
use serde_json::json;

use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::context_switch::ThreadContextSwitchData;

pub struct Cpus {
    start_time: Timestamp,
    process_handle: ProcessHandle,
    combined_thread_handle: ThreadHandle,
    cpus: Vec<Cpu>,
    idle_frame_label: FrameInfo,
}

pub struct Cpu {
    pub name: String,
    pub thread_handle: ThreadHandle,
    pub context_switch_data: ThreadContextSwitchData,
    pub current_tid: Option<(i32, StringHandle, u64)>,
}

impl Cpu {
    pub fn new(cpu_index: usize, thread_handle: ThreadHandle) -> Self {
        Self {
            name: format!("CPU {cpu_index}"),
            thread_handle,
            context_switch_data: Default::default(),
            current_tid: None,
        }
    }

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
            // eprintln!("Missing switch-out (noticed during switch-in) on {}: {previous_tid}, {switch_in_timestamp}", self.name);
            let name = profile.get_string(previous_thread_name).to_string();
            let start_timestamp = converter.convert_time(switch_in_timestamp);
            let end_timestamp = converter.convert_time(timestamp);
            let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
            for thread_handle in thread_handles {
                profile.add_marker(
                    *thread_handle,
                    CategoryHandle::OTHER,
                    &self.name,
                    ThreadNameMarkerForCpuTrack(name.clone()),
                    timing.clone(),
                );
            }
        }
    }

    pub fn notify_switch_out(
        &mut self,
        tid: i32,
        timestamp: u64,
        converter: &TimestampConverter,
        thread_handles: &[ThreadHandle],
        profile: &mut Profile,
    ) {
        let previous_tid = self.current_tid.take();
        if let Some((previous_tid, previous_thread_name, switch_in_timestamp)) = previous_tid {
            let name = profile.get_string(previous_thread_name).to_string();
            let start_timestamp = converter.convert_time(switch_in_timestamp);
            let end_timestamp = converter.convert_time(timestamp);
            let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
            for thread_handle in thread_handles {
                profile.add_marker(
                    *thread_handle,
                    CategoryHandle::OTHER,
                    &self.name,
                    ThreadNameMarkerForCpuTrack(name.clone()),
                    timing.clone(),
                );
            }
            if previous_tid != tid {
                // eprintln!("Missing switch-out (noticed during switch-out) on {}: {previous_tid}, {switch_in_timestamp}", self.name);
                // eprintln!(
                //     "Missing switch-in (noticed during switch-out) on {}: {tid}, {timestamp}",
                //     self.name
                // );
            }
        } else {
            // eprintln!(
            //     "Missing switch-in (noticed during switch-out) on {}: {tid}, {timestamp}",
            //     self.name
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
            let cpu = Cpu::new(i, thread);
            profile.set_thread_name(thread, &cpu.name);
            self.cpus.push(cpu);
        }
        &mut self.cpus[cpu]
    }
}

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct ThreadNameMarkerForCpuTrack(pub String);

impl ProfilerMarker for ThreadNameMarkerForCpuTrack {
    const MARKER_TYPE_NAME: &'static str = "ContextSwitch";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "name": self.0
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}"),
            tooltip_label: Some("{marker.data.name}"),
            table_label: Some("{marker.name} - {marker.data.name}"),
            fields: vec![MarkerSchemaField::Dynamic(MarkerDynamicField {
                key: "thread",
                label: "Thread",
                format: MarkerFieldFormat::String,
                searchable: true,
            })],
        }
    }
}
