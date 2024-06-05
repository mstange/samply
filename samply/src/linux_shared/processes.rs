use std::collections::hash_map::Entry;
use std::collections::HashMap;

use framehop::Unwinder;
use fxprof_processed_profile::{CategoryColor, Profile, Timestamp};

use super::process::Process;
use super::process_threads::make_thread_label_frame;
use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::jit_function_recycler::JitFunctionRecycler;
use crate::shared::process_sample_data::ProcessSampleData;
use crate::shared::recycling::{ProcessRecycler, ProcessRecyclingData, ThreadRecycler};
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::unresolved_samples::UnresolvedStacks;

pub struct Processes<U>
where
    U: Unwinder + Default,
{
    processes_by_pid: HashMap<i32, Process<U>>,

    /// Some() if a thread should be merged into a previously exited
    /// thread of the same name.
    process_recycler: Option<ProcessRecycler>,

    /// The sample data for all removed processes.
    process_sample_datas: Vec<ProcessSampleData>,

    /// Whether aux files (like jitdump) should be unlinked on open
    unlink_aux_data: bool,
}

impl<U> Processes<U>
where
    U: Unwinder + Default,
{
    pub fn new(allow_reuse: bool, unlink_aux_data: bool) -> Self {
        let process_recycler = if allow_reuse {
            Some(ProcessRecycler::new())
        } else {
            None
        };
        Self {
            processes_by_pid: HashMap::new(),
            process_recycler,
            process_sample_datas: Vec::new(),
            unlink_aux_data,
        }
    }

    pub fn recycle_or_get_new(
        &mut self,
        pid: i32,
        name: Option<String>,
        start_time: Timestamp,
        profile: &mut Profile,
    ) -> &mut Process<U> {
        match self.processes_by_pid.entry(pid) {
            Entry::Vacant(entry) => {
                if let (Some(process_recycler), Some(name_ref)) =
                    (self.process_recycler.as_mut(), name.as_deref())
                {
                    if let Some(ProcessRecyclingData {
                        process_handle,
                        main_thread_recycling_data,
                        thread_recycler,
                        jit_function_recycler,
                    }) = process_recycler.recycle_by_name(name_ref)
                    {
                        let (main_thread_handle, main_thread_label_frame) =
                            main_thread_recycling_data;
                        let process = Process::new(
                            pid,
                            process_handle,
                            main_thread_handle,
                            main_thread_label_frame,
                            name,
                            Some(thread_recycler),
                            Some(jit_function_recycler),
                            self.unlink_aux_data,
                        );
                        return entry.insert(process);
                    }
                }

                let fallback_name = format!("<{pid}>");
                let process_handle = profile.add_process(
                    name.as_deref().unwrap_or(&fallback_name),
                    pid as u32,
                    start_time,
                );
                let main_thread_handle =
                    profile.add_thread(process_handle, pid as u32, start_time, true);
                if let Some(name) = name.as_deref() {
                    profile.set_thread_name(main_thread_handle, name);
                }
                let main_thread_label_frame =
                    make_thread_label_frame(profile, name.as_deref(), pid, pid);
                let (thread_recycler, jit_function_recycler) = if self.process_recycler.is_some() {
                    (
                        Some(ThreadRecycler::new()),
                        Some(JitFunctionRecycler::default()),
                    )
                } else {
                    (None, None)
                };
                let process = Process::new(
                    pid,
                    process_handle,
                    main_thread_handle,
                    main_thread_label_frame,
                    name,
                    thread_recycler,
                    jit_function_recycler,
                    self.unlink_aux_data,
                );
                entry.insert(process)
            }
            Entry::Occupied(entry) => {
                // Why do we have a thread for this TID already? It should be a new thread.
                // Two options come to mind:
                //  - The TID got reused, and we missed an EXIT event for the old thread.
                //  - Or the FORK for this thread wasn't actually the first event that we
                //    see for this thread.
                //
                // If we're in the latter case, we may have given this process / thread a
                // start time that's too early. Let's adjust the start time if this process
                // doesn't have any samples yet.
                let process = entry.into_mut();
                if process.threads.main_thread.last_sample_timestamp.is_none() {
                    profile.set_process_start_time(process.profile_process, start_time);
                    profile.set_thread_start_time(
                        process.threads.main_thread.profile_thread,
                        start_time,
                    );
                }
                process
            }
        }
    }

    pub fn get_by_pid(&mut self, pid: i32, profile: &mut Profile) -> &mut Process<U> {
        self.processes_by_pid.entry(pid).or_insert_with(|| {
            let fake_start_time = Timestamp::from_millis_since_reference(0.0);
            let process_handle =
                profile.add_process(&format!("<{pid}>"), pid as u32, fake_start_time);
            let main_thread_handle =
                profile.add_thread(process_handle, pid as u32, fake_start_time, true);
            let main_thread_label_frame = make_thread_label_frame(profile, None, pid, pid);
            let (thread_recycler, jit_function_recycler) = if self.process_recycler.is_some() {
                (
                    Some(ThreadRecycler::new()),
                    Some(JitFunctionRecycler::default()),
                )
            } else {
                (None, None)
            };
            Process::new(
                pid,
                process_handle,
                main_thread_handle,
                main_thread_label_frame,
                None, // no name
                thread_recycler,
                jit_function_recycler,
                self.unlink_aux_data,
            )
        })
    }

    pub fn remove(
        &mut self,
        pid: i32,
        time: Timestamp,
        profile: &mut Profile,
        jit_category_manager: &mut JitCategoryManager,
        timestamp_converter: &TimestampConverter,
    ) {
        let Some(mut process) = self.processes_by_pid.remove(&pid) else {
            return;
        };

        process.notify_dead(time, profile);

        let (process_sample_data, process_recycling_data) =
            process.finish(profile, jit_category_manager, timestamp_converter);
        if !process_sample_data.is_empty() {
            self.process_sample_datas.push(process_sample_data);
        }

        if let (Some((name, process_recycling_data)), Some(process_recycler)) =
            (process_recycling_data, self.process_recycler.as_mut())
        {
            process_recycler.add_to_pool(&name, process_recycling_data);
        }
    }

    pub fn rename_process(
        &mut self,
        pid: i32,
        timestamp: Timestamp,
        name: String,
        profile: &mut Profile,
    ) {
        match self.processes_by_pid.entry(pid) {
            Entry::Vacant(_) => {
                self.recycle_or_get_new(pid, Some(name), timestamp, profile);
            }
            Entry::Occupied(mut entry) => {
                let process = entry.get_mut();
                if process.name.as_deref() == Some(&name) {
                    return;
                }

                if let Some(process_recycler) = self.process_recycler.as_mut() {
                    let Some(process_recycling_data) = process_recycler.recycle_by_name(&name)
                    else {
                        return;
                    };
                    let (old_recycling_data, old_name) =
                        process.rename_with_recycling(name, process_recycling_data);
                    if let Some(old_name) = old_name {
                        process_recycler.add_to_pool(&old_name, old_recycling_data);
                    }
                } else {
                    let main_thread_label_frame =
                        make_thread_label_frame(profile, Some(&name), pid, pid);
                    process.rename_without_recycling(name, main_thread_label_frame, profile);
                }
            }
        }
    }

    pub fn finish(
        mut self,
        profile: &mut Profile,
        unresolved_stacks: &UnresolvedStacks,
        jit_category_manager: &mut JitCategoryManager,
        timestamp_converter: &TimestampConverter,
    ) {
        // Gather the ProcessSampleData from any processes which are still alive at the end of profiling.
        for process in self.processes_by_pid.into_values() {
            let (process_sample_data, _process_recycling_data) =
                process.finish(profile, jit_category_manager, timestamp_converter);
            if !process_sample_data.is_empty() {
                self.process_sample_datas.push(process_sample_data);
            }
        }

        let user_category = profile.add_category("User", CategoryColor::Yellow).into();
        let kernel_category = profile.add_category("Kernel", CategoryColor::Orange).into();
        let mut stack_frame_scratch_buf = Vec::new();
        for process_sample_data in self.process_sample_datas {
            process_sample_data.flush_samples_to_profile(
                profile,
                user_category,
                kernel_category,
                &mut stack_frame_scratch_buf,
                unresolved_stacks,
            );
        }
    }
}
