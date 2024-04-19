use fxprof_processed_profile::{ProcessHandle, Profile, ThreadHandle, Timestamp};

use super::context_switch::ThreadContextSwitchData;

pub struct Cpus {
    start_time: Timestamp,
    process_handle: ProcessHandle,
    combined_thread_handle: ThreadHandle,
    cpus: Vec<Cpu>,
}

pub struct Cpu {
    pub thread_handle: ThreadHandle,
    pub context_switch_data: ThreadContextSwitchData,
}

impl Cpu {
    pub fn new(thread_handle: ThreadHandle) -> Self {
        Self {
            thread_handle,
            context_switch_data: Default::default(),
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
            profile.set_thread_name(thread, &format!("CPU {i}"));
            self.cpus.push(Cpu::new(thread));
        }
        &mut self.cpus[cpu]
    }
}
