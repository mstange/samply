use std::collections::hash_map::Entry;

use fxprof_processed_profile::{
    CategoryHandle, Frame, FrameFlags, FrameInfo, ProcessHandle, Profile, ThreadHandle, Timestamp,
};

use super::thread::Thread;
use crate::shared::recycling::ThreadRecycler;
use crate::shared::types::FastHashMap;

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
        main_thread_label_frame: FrameInfo,
        name: Option<String>,
        thread_recycler: Option<ThreadRecycler>,
    ) -> Self {
        Self {
            pid,
            profile_process: process_handle,
            main_thread: Thread::new(main_thread_handle, main_thread_label_frame, name),
            threads_by_tid: Default::default(),
            thread_recycler,
        }
    }

    pub fn rename_process_with_recycling(
        &mut self,
        name: String,
        process_handle: ProcessHandle,
        main_thread_recycling_data: (ThreadHandle, FrameInfo),
        thread_recycler: ThreadRecycler,
    ) -> (ThreadRecycler, (ThreadHandle, FrameInfo)) {
        let _old_process_handle = std::mem::replace(&mut self.profile_process, process_handle);
        let (_old_name, old_main_thread_recycling_data) = self
            .main_thread
            .rename_with_recycling(name, main_thread_recycling_data);
        let old_thread_recycler =
            std::mem::replace(&mut self.thread_recycler, Some(thread_recycler));
        (
            old_thread_recycler.expect("thread_recycler should be Some"),
            old_main_thread_recycling_data,
        )
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
                if let (Some(name), Some(thread_recycler)) = (&name, self.thread_recycler.as_mut())
                {
                    if let Some((thread_handle, thread_label_frame)) =
                        thread_recycler.recycle_by_name(name)
                    {
                        let thread =
                            Thread::new(thread_handle, thread_label_frame, Some(name.clone()));
                        return entry.insert(thread);
                    }
                }

                let thread_handle =
                    profile.add_thread(self.profile_process, tid as u32, start_time, false);
                if let Some(name) = &name {
                    profile.set_thread_name(thread_handle, name);
                }
                let thread_label_frame =
                    make_thread_label_frame(profile, name.as_deref(), self.pid, tid);
                let thread = Thread::new(thread_handle, thread_label_frame, name);
                entry.insert(thread)
            }
            Entry::Occupied(entry) => {
                // Why do we have a thread for this TID already? It should be a new thread.
                // Two options come to mind:
                //  - The TID got reused, and we missed an EXIT event for the old thread.
                //  - Or the FORK for this thread wasn't actually the first event that we
                //    see for this thread.
                //
                // If we're in the latter case, we may have given this thread a start time
                // that's too early. Let's adjust the start time if the thread doesn't have
                // any samples yet.
                // In particular, simpleperf is known to emit extra COMM (and MMAP2) events with
                // backdated timestamps that can be before a thread's creation.
                let thread = entry.into_mut();
                if thread.last_sample_timestamp.is_none() {
                    profile.set_thread_start_time(thread.profile_thread, start_time);
                }
                thread
            }
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
                let thread = entry.get_mut();
                if thread.name.as_deref() == Some(&name) {
                    return;
                }

                if let Some(thread_recycler) = self.thread_recycler.as_mut() {
                    if let Some(thread_recycling_data) = thread_recycler.recycle_by_name(&name) {
                        let (old_name, old_thread_recycling_data) =
                            thread.rename_with_recycling(name, thread_recycling_data);
                        if let Some(old_name) = old_name {
                            thread_recycler.add_to_pool(&old_name, old_thread_recycling_data);
                        }
                    }
                } else {
                    let thread_label_frame =
                        make_thread_label_frame(profile, Some(&name), self.pid, tid);
                    thread.rename_without_recycling(name, thread_label_frame, profile);
                }
            }
        }
    }

    /// Called when a process has exited, before finish(). Not called if the process
    /// is still alive at the end of the profiling run.
    pub fn notify_process_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        for (_tid, mut thread) in self.threads_by_tid.drain() {
            thread.notify_dead(end_time, profile);

            let (name, thread_recycling_data) = thread.finish();

            if let (Some(name), Some(thread_recycler)) = (name, self.thread_recycler.as_mut()) {
                thread_recycler.add_to_pool(&name, thread_recycling_data);
            }
        }

        self.main_thread.notify_dead(end_time, profile);
    }

    /// Called when the process has exited, or at the end of profiling. Called after notify_process_dead.
    pub fn finish(self) -> (Option<ThreadRecycler>, (ThreadHandle, FrameInfo)) {
        let (_main_thread_name, main_thread_recycling_data) = self.main_thread.finish();
        (self.thread_recycler, main_thread_recycling_data)
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
            let thread_label_frame = make_thread_label_frame(profile, None, self.pid, tid);
            Thread {
                profile_thread,
                context_switch_data: Default::default(),
                last_sample_timestamp: None,
                off_cpu_stack: None,
                name: None,
                thread_label_frame,
            }
        })
    }

    pub fn remove_non_main_thread(&mut self, tid: i32, time: Timestamp, profile: &mut Profile) {
        let Some(mut thread) = self.threads_by_tid.remove(&tid) else {
            return;
        };

        thread.notify_dead(time, profile);

        let (name, thread_recylcing_data) = thread.finish();

        if let (Some(name), Some(thread_recycler)) = (name, self.thread_recycler.as_mut()) {
            thread_recycler.add_to_pool(&name, thread_recylcing_data);
        }
    }
}

pub fn make_thread_label_frame(
    profile: &mut Profile,
    name: Option<&str>,
    pid: i32,
    tid: i32,
) -> FrameInfo {
    let s = match name {
        Some(name) => format!("{name} (pid: {pid}, tid: {tid})"),
        None => format!("Thread {tid} (pid: {pid}, tid: {tid})"),
    };
    let thread_label = profile.intern_string(&s);
    FrameInfo {
        frame: Frame::Label(thread_label),
        category_pair: CategoryHandle::OTHER.into(),
        flags: FrameFlags::empty(),
    }
}
