use fxprof_processed_profile::ThreadHandle;

use std::fmt::Debug;

use super::context_switch::ThreadContextSwitchData;

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
    pub fn on_remove(&mut self) {
        self.context_switch_data = Default::default();
        self.last_sample_timestamp = None;
        self.off_cpu_stack = None;
    }

    pub fn reset_for_reuse(&mut self, _tid: i32) {}
}
