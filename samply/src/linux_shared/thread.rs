use fxprof_processed_profile::{Profile, ThreadHandle, Timestamp};

use std::fmt::Debug;

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
}

impl Thread {
    pub fn new(thread_handle: ThreadHandle) -> Self {
        Self {
            profile_thread: thread_handle,
            context_switch_data: Default::default(),
            last_sample_timestamp: None,
            off_cpu_stack: None,
            name: None,
        }
    }

    pub fn swap_thread_handle(&mut self, thread_handle: ThreadHandle) -> ThreadHandle {
        std::mem::replace(&mut self.profile_thread, thread_handle)
    }

    pub fn set_name(&mut self, name: String, profile: &mut Profile) {
        profile.set_thread_name(self.profile_thread, &name);
        self.name = Some(name);
    }

    pub fn notify_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        profile.set_thread_end_time(self.profile_thread, end_time);
    }

    pub fn finish(self) -> (Option<String>, ThreadHandle) {
        (self.name, self.profile_thread)
    }
}
