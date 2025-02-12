use std::fmt::Debug;

use fxprof_processed_profile::{Profile, StringHandle, ThreadHandle, Timestamp};

use crate::shared::context_switch::ThreadContextSwitchData;
use crate::shared::unresolved_samples::UnresolvedStackHandle;

#[derive(Debug)]
pub struct Thread {
    pub profile_thread: ThreadHandle,
    pub context_switch_data: ThreadContextSwitchData,
    pub last_sample_timestamp: Option<u64>,

    /// Some() between sched_switch and the next context switch IN
    ///
    /// Refers to a stack in the containing Process's UnresolvedSamples stack table.
    pub off_cpu_stack: Option<UnresolvedStackHandle>,
    pub name: Option<String>,
    pub thread_label: StringHandle,
}

impl Thread {
    pub fn new(
        thread_handle: ThreadHandle,
        thread_label: StringHandle,
        name: Option<String>,
    ) -> Self {
        Self {
            profile_thread: thread_handle,
            context_switch_data: Default::default(),
            last_sample_timestamp: None,
            off_cpu_stack: None,
            name,
            thread_label,
        }
    }

    pub fn rename_with_recycling(
        &mut self,
        name: String,
        (thread_handle, thread_label): (ThreadHandle, StringHandle),
    ) -> (Option<String>, (ThreadHandle, StringHandle)) {
        let old_thread_handle = std::mem::replace(&mut self.profile_thread, thread_handle);
        let old_thread_label = std::mem::replace(&mut self.thread_label, thread_label);
        let old_name = std::mem::replace(&mut self.name, Some(name));
        (old_name, (old_thread_handle, old_thread_label))
    }

    pub fn rename_without_recycling(
        &mut self,
        name: String,
        thread_label: StringHandle,
        profile: &mut Profile,
    ) {
        profile.set_thread_name(self.profile_thread, &name);
        self.thread_label = thread_label;
        self.name = Some(name);
    }

    pub fn notify_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        profile.set_thread_end_time(self.profile_thread, end_time);
    }

    pub fn finish(self) -> (Option<String>, (ThreadHandle, StringHandle)) {
        (self.name, (self.profile_thread, self.thread_label))
    }
}
