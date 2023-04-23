use fxprof_processed_profile::{ProcessHandle, Profile, Timestamp};

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

use super::thread::Thread;

pub struct ProcessThreads {
    pub pid: i32,
    pub profile_process: ProcessHandle,
    pub main_thread: Thread,
    pub threads_by_tid: HashMap<i32, Thread>,
    pub ended_threads_for_reuse_by_name: HashMap<String, VecDeque<Thread>>,
}

impl ProcessThreads {
    pub fn prepare_for_reuse(&mut self) {
        for (_tid, mut thread) in self.threads_by_tid.drain() {
            thread.on_remove();

            if let Some(name) = thread.name.as_deref() {
                self.ended_threads_for_reuse_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push_back(thread);
            }
        }
    }

    pub fn attempt_thread_reuse(&mut self, tid: i32, name: &str) -> Option<&mut Thread> {
        if let Entry::Vacant(entry) = self.threads_by_tid.entry(tid) {
            if let Some(threads_of_same_name) = self.ended_threads_for_reuse_by_name.get_mut(name) {
                let mut thread = threads_of_same_name
                    .pop_front()
                    .expect("We only have non-empty VecDeques in this HashMap");
                if threads_of_same_name.is_empty() {
                    self.ended_threads_for_reuse_by_name.remove(name);
                }
                thread.reset_for_reuse(tid);
                return Some(entry.insert(thread));
            }
        }
        None
    }

    pub fn get_main_thread(&mut self) -> &mut Thread {
        &mut self.main_thread
    }

    pub fn get_thread_by_tid(&mut self, tid: i32, profile: &mut Profile) -> &mut Thread {
        if tid == self.pid {
            return &mut self.main_thread;
        }
        self.threads_by_tid.entry(tid).or_insert_with(|| {
            let profile_thread = profile.add_thread(
                self.profile_process,
                tid as u32,
                Timestamp::from_millis_since_reference(0.0),
                false,
            );
            Thread {
                profile_thread,
                context_switch_data: Default::default(),
                last_sample_timestamp: None,
                off_cpu_stack: None,
                name: None,
            }
        })
    }

    pub fn remove_non_main_thread(
        &mut self,
        tid: i32,
        time: Timestamp,
        allow_reuse: bool,
        profile: &mut Profile,
    ) {
        let Some(mut thread) = self.threads_by_tid.remove(&tid) else { return };
        profile.set_thread_end_time(thread.profile_thread, time);

        thread.on_remove();

        if allow_reuse {
            if let Some(name) = thread.name.as_deref() {
                self.ended_threads_for_reuse_by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push_back(thread);
            }
        }
    }
}
