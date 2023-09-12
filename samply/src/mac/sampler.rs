use crossbeam_channel::Receiver;
use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, Profile, ReferenceTimestamp};
use mach::port::mach_port_t;

use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

use crate::shared::recording_props::{ConversionProps, RecordingProps};
use crate::shared::recycling::ProcessRecycler;
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::unresolved_samples::UnresolvedStacks;

use super::error::SamplingError;
use super::task_profiler::TaskProfiler;
use super::time::get_monotonic_timestamp;

#[derive(Debug, Clone)]
pub struct TaskInit {
    pub start_time_mono: u64,
    pub task: mach_port_t,
    pub pid: u32,
    pub jitdump_path_receiver: Receiver<PathBuf>,
}

pub struct Sampler {
    command_name: String,
    task_receiver: Receiver<TaskInit>,
    recording_props: RecordingProps,
    conversion_props: ConversionProps,
}

impl Sampler {
    pub fn new(
        command: String,
        task_receiver: Receiver<TaskInit>,
        recording_props: RecordingProps,
        conversion_props: ConversionProps,
    ) -> Self {
        let command_name = Path::new(&command)
            .components()
            .next_back()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .to_string();

        Sampler {
            command_name,
            task_receiver,
            recording_props,
            conversion_props,
        }
    }

    pub fn run(self) -> Result<Profile, SamplingError> {
        let reference_mono = get_monotonic_timestamp();
        let reference_system_time = SystemTime::now();

        let timestamp_converter = TimestampConverter {
            reference_raw: reference_mono,
            raw_to_ns_factor: 1,
        };

        let mut profile = Profile::new(
            &self.conversion_props.profile_name,
            ReferenceTimestamp::from_system_time(reference_system_time),
            self.recording_props.interval.into(),
        );

        let mut jit_category_manager =
            crate::shared::jit_category_manager::JitCategoryManager::new();

        let default_category =
            CategoryPairHandle::from(profile.add_category("User", CategoryColor::Yellow));

        let root_task_init = match self.task_receiver.recv() {
            Ok(task_init) => task_init,
            Err(_) => {
                // The sender went away. No profiling today.
                return Err(SamplingError::CouldNotObtainRootTask);
            }
        };
        let mut process_recycler = if self.conversion_props.merge_threads {
            Some(ProcessRecycler::new())
        } else {
            None
        };

        let root_task = TaskProfiler::new(
            root_task_init,
            timestamp_converter,
            &self.command_name,
            &mut profile,
            process_recycler.as_mut(),
        )
        .expect("couldn't create root TaskProfiler");

        let mut process_sample_datas = Vec::new();
        let mut stack_scratch_buffer = Vec::new();
        let mut live_tasks = vec![root_task];
        let mut unwinder_cache = Default::default();
        let mut unresolved_stacks = UnresolvedStacks::default();
        let mut last_sleep_overshoot = 0;

        loop {
            loop {
                let task_init = if !live_tasks.is_empty() {
                    // Poll to see if there are any new tasks we should add. If no new tasks are available,
                    // this completes immediately.
                    self.task_receiver.try_recv().ok()
                } else {
                    // All tasks we know about are dead.
                    // Wait for a little more in case one of the just-ended tasks spawned a new task.
                    let all_dead_timeout = Duration::from_secs_f32(0.5);
                    self.task_receiver.recv_timeout(all_dead_timeout).ok()
                };
                let Some(task_init) = task_init else { break };
                if let Ok(new_task) = TaskProfiler::new(
                    task_init,
                    timestamp_converter,
                    &self.command_name,
                    &mut profile,
                    process_recycler.as_mut(),
                ) {
                    live_tasks.push(new_task);
                } else {
                    // The task is probably already dead again. We get here for tasks which are
                    // very short-lived.
                }
            }

            if live_tasks.is_empty() {
                eprintln!("All tasks terminated.");
                break;
            }

            let sample_mono = get_monotonic_timestamp();
            if let Some(time_limit) = self.recording_props.time_limit {
                if sample_mono - reference_mono >= time_limit.as_nanos() as u64 {
                    // Time limit reached.
                    break;
                }
            }

            let sample_timestamp = timestamp_converter.convert_time(sample_mono);

            let mut tasks = Vec::with_capacity(live_tasks.capacity());
            mem::swap(&mut live_tasks, &mut tasks);
            for mut task in tasks.into_iter() {
                task.check_jitdump(&mut profile, &mut jit_category_manager);
                let still_alive = task.sample(
                    sample_timestamp,
                    sample_mono,
                    &mut unwinder_cache,
                    &mut profile,
                    &mut stack_scratch_buffer,
                    &mut unresolved_stacks,
                    self.conversion_props.fold_recursive_prefix,
                )?;
                if still_alive {
                    live_tasks.push(task);
                } else {
                    task.notify_dead(sample_timestamp, &mut profile);
                    let (process_sample_data, process_recycling_data) =
                        task.finish(&mut jit_category_manager, &mut profile);

                    process_sample_datas.push(process_sample_data);

                    if let (Some(process_recycler), Some((process_name, process_recycling_data))) =
                        (process_recycler.as_mut(), process_recycling_data)
                    {
                        process_recycler.add_to_pool(&process_name, process_recycling_data);
                    }
                }
            }

            let intended_wakeup_time =
                sample_mono + self.recording_props.interval.as_nanos() as u64;
            let before_sleep = get_monotonic_timestamp();
            let indended_wait_time = intended_wakeup_time.saturating_sub(before_sleep);
            let sleep_time = indended_wait_time.saturating_sub(last_sleep_overshoot);
            thread::sleep(Duration::from_nanos(sleep_time));
            let actual_sleep_duration = get_monotonic_timestamp() - before_sleep;
            last_sleep_overshoot = actual_sleep_duration.saturating_sub(sleep_time);
        }

        // Gather the sample data from the remaining live tasks.
        // `live_tasks` can be non-empty if we stopped profiling before all tasks ended,
        // for example because the time limit was reached,
        for task in live_tasks.into_iter() {
            let (process_sample_data, _process_recycling_data) =
                task.finish(&mut jit_category_manager, &mut profile);
            process_sample_datas.push(process_sample_data);
        }

        let mut stack_frame_scratch_buf = Vec::new();
        for process_sample_data in process_sample_datas {
            process_sample_data.flush_samples_to_profile(
                &mut profile,
                default_category,
                default_category,
                &mut stack_frame_scratch_buf,
                &unresolved_stacks,
                &[],
            );
        }

        Ok(profile)
    }
}
