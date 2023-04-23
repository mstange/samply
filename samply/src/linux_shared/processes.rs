use framehop::{Module, Unwinder};
use fxprof_processed_profile::{CategoryColor, Profile, Timestamp};

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

use super::process::Process;
use super::process_threads::ProcessThreads;
use super::thread::Thread;

use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::jit_function_recycler::JitFunctionRecycler;
use crate::shared::jitdump_manager::JitDumpManager;
use crate::shared::process_sample_data::ProcessSampleData;
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::unresolved_samples::UnresolvedStacks;

pub struct Processes<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    processes_by_pid: HashMap<i32, Process<U>>,
    ended_processes_for_reuse_by_name: HashMap<String, VecDeque<Process<U>>>,

    /// The sample data for all removed processes.
    process_sample_datas: Vec<ProcessSampleData>,

    allow_reuse: bool,
}

impl<U> Processes<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    pub fn new(allow_reuse: bool) -> Self {
        Self {
            processes_by_pid: HashMap::new(),
            ended_processes_for_reuse_by_name: HashMap::new(),
            process_sample_datas: Vec::new(),
            allow_reuse,
        }
    }

    pub fn attempt_reuse(&mut self, pid: i32, name: &str) -> Option<&mut Process<U>> {
        if let Entry::Vacant(entry) = self.processes_by_pid.entry(pid) {
            if let Some(processes_of_same_name) =
                self.ended_processes_for_reuse_by_name.get_mut(name)
            {
                let mut process = processes_of_same_name
                    .pop_front()
                    .expect("We only have non-empty VecDeques in this HashMap");
                if processes_of_same_name.is_empty() {
                    self.ended_processes_for_reuse_by_name.remove(name);
                }
                process.reset_for_reuse(pid);
                return Some(entry.insert(process));
            }
        }
        None
    }

    pub fn get_by_pid(&mut self, pid: i32, profile: &mut Profile) -> &mut Process<U> {
        self.processes_by_pid.entry(pid).or_insert_with(|| {
            let name = format!("<{pid}>");
            let handle = profile.add_process(
                &name,
                pid as u32,
                Timestamp::from_millis_since_reference(0.0),
            );
            let profile_thread = profile.add_thread(
                handle,
                pid as u32,
                Timestamp::from_millis_since_reference(0.0),
                true,
            );
            let main_thread = Thread {
                profile_thread,
                context_switch_data: Default::default(),
                last_sample_timestamp: None,
                off_cpu_stack: None,
                name: None,
            };
            let jit_function_recycler = if self.allow_reuse {
                Some(JitFunctionRecycler::default())
            } else {
                None
            };
            Process {
                profile_process: handle,
                unwinder: U::default(),
                jitdump_manager: JitDumpManager::new_for_process(profile_thread),
                lib_mapping_ops: Default::default(),
                name: None,
                pid,
                threads: ProcessThreads {
                    pid,
                    profile_process: handle,
                    main_thread,
                    threads_by_tid: HashMap::new(),
                    ended_threads_for_reuse_by_name: HashMap::new(),
                },
                jit_function_recycler,
                unresolved_samples: Default::default(),
                prev_mm_filepages_size: 0,
                prev_mm_anonpages_size: 0,
                prev_mm_swapents_size: 0,
                prev_mm_shmempages_size: 0,
                mem_counter: None,
            }
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
        let Some(mut process) = self.processes_by_pid.remove(&pid) else { return };
        profile.set_process_end_time(process.profile_process, time);

        let process_sample_data = process.on_remove(
            self.allow_reuse,
            profile,
            jit_category_manager,
            timestamp_converter,
        );
        if !process_sample_data.is_empty() {
            self.process_sample_datas.push(process_sample_data);
        }

        if self.allow_reuse {
            if let Some(name) = process.name.as_deref() {
                self.ended_processes_for_reuse_by_name
                    .entry(name.to_string())
                    .or_default()
                    .push_back(process);
            }
        }
    }

    pub fn finish(
        mut self,
        profile: &mut Profile,
        unresolved_stacks: &UnresolvedStacks,
        event_names: &[String],
        jit_category_manager: &mut JitCategoryManager,
        timestamp_converter: &TimestampConverter,
    ) {
        // Gather the ProcessSampleData from any processes which are still alive at the end of profiling.
        for mut process in self.processes_by_pid.into_values() {
            let process_sample_data = process.on_remove(
                self.allow_reuse,
                profile,
                jit_category_manager,
                timestamp_converter,
            );
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
                event_names,
            );
        }
    }
}
