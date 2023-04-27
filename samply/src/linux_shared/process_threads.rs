use fxprof_processed_profile::{ProcessHandle, Profile, ThreadHandle, Timestamp};

use std::collections::hash_map::Entry;

use crate::shared::recycling::ThreadRecycler;
use crate::shared::types::FastHashMap;

use super::thread::Thread;

pub struct ProcessThreads {
    pub pid: i32,
    pub profile_process: ProcessHandle,
    pub main_thread: Thread,
    pub threads_by_tid: FastHashMap<i32, Thread>,
    pub thread_recycler: Option<ThreadRecycler>,
}

impl ProcessThreads {
    pub fn new(
        pid: i32,
        process_handle: ProcessHandle,
        main_thread_handle: ThreadHandle,
        thread_recycler: Option<ThreadRecycler>,
    ) -> Self {
        Self {
            pid,
            profile_process: process_handle,
            main_thread: Thread::new(main_thread_handle),
            threads_by_tid: Default::default(),
            thread_recycler,
        }
    }

    pub fn swap_recycling_data(
        &mut self,
        process_handle: ProcessHandle,
        main_thread_handle: ThreadHandle,
        thread_recycler: ThreadRecycler,
    ) -> Option<(ThreadHandle, ThreadRecycler)> {
        let _old_process_handle = std::mem::replace(&mut self.profile_process, process_handle);
        let old_main_thread_handle = self.main_thread.swap_thread_handle(main_thread_handle);
        let old_thread_recycler =
            std::mem::replace(&mut self.thread_recycler, Some(thread_recycler));
        Some((old_main_thread_handle, old_thread_recycler?))
    }

    pub fn recycle_or_get_new_thread(
        &mut self,
        tid: i32,
        name: Option<String>,
        start_time: Timestamp,
        profile: &mut Profile,
    ) -> &mut Thread {
        if tid == self.pid {
            return &mut self.main_thread;
        }
        match self.threads_by_tid.entry(tid) {
            Entry::Vacant(entry) => {
                if let (Some(name), Some(thread_recycler)) = (name, self.thread_recycler.as_mut()) {
                    if let Some(thread_handle) = thread_recycler.recycle_by_name(&name) {
                        let thread = Thread::new(thread_handle);
                        return entry.insert(thread);
                    }
                }

                let thread_handle =
                    profile.add_thread(self.profile_process, tid as u32, start_time, false);
                let thread = Thread::new(thread_handle);
                entry.insert(thread)
            }
            Entry::Occupied(entry) => entry.into_mut(),
        }
    }

    pub fn rename_non_main_thread(
        &mut self,
        tid: i32,
        timestamp: Timestamp,
        name: String,
        profile: &mut Profile,
    ) {
        if tid == self.pid {
            return;
        }
        match self.threads_by_tid.entry(tid) {
            Entry::Vacant(_) => {
                self.recycle_or_get_new_thread(tid, Some(name), timestamp, profile);
            }
            Entry::Occupied(mut entry) => {
                if entry.get().name.as_deref() == Some(&name) {
                    return;
                }

                if let Some(thread_recycler) = self.thread_recycler.as_mut() {
                    if let Some(recycled_thread_handle) = thread_recycler.recycle_by_name(&name) {
                        let old_thread_handle =
                            entry.get_mut().swap_thread_handle(recycled_thread_handle);
                        if let Some(old_name) = entry.get().name.as_deref() {
                            thread_recycler.add_to_pool(old_name, old_thread_handle);
                        }
                    }
                }

                entry.get_mut().set_name(name, profile);
            }
        }
    }

    /// Called when a process has exited, before finish(). Not called if the process
    /// is still alive at the end of the profiling run.
    pub fn notify_process_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        for (_tid, mut thread) in self.threads_by_tid.drain() {
            thread.notify_dead(end_time, profile);

            let (name, thread_handle) = thread.finish();

            if let (Some(name), Some(thread_recycler)) = (name, self.thread_recycler.as_mut()) {
                thread_recycler.add_to_pool(&name, thread_handle);
            }
        }

        self.main_thread.notify_dead(end_time, profile);
    }
    /// Called when the process has exited, or at the end of profiling. Called after notify_process_dead.
    pub fn finish(self) -> (ThreadHandle, Option<ThreadRecycler>) {
        let (_main_thread_name, main_thread_handle) = self.main_thread.finish();
        (main_thread_handle, self.thread_recycler)
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

    pub fn remove_non_main_thread(&mut self, tid: i32, time: Timestamp, profile: &mut Profile) {
        let Some(mut thread) = self.threads_by_tid.remove(&tid) else { return };

        thread.notify_dead(time, profile);

        let (name, thread_handle) = thread.finish();

        if let (Some(name), Some(thread_recycler)) = (name, self.thread_recycler.as_mut()) {
            thread_recycler.add_to_pool(&name, thread_handle);
        }
    }
}
