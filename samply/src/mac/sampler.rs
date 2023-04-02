use crossbeam_channel::Receiver;
use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, Profile, ReferenceTimestamp};
use mach::port::mach_port_t;

use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::unresolved_samples::UnresolvedStacks;

use super::error::SamplingError;
use super::task_profiler::TaskProfiler;
use super::time::get_monotonic_timestamp;

#[derive(Debug, Clone)]
pub struct TaskInit {
    pub start_time: u64,
    pub task: mach_port_t,
    pub pid: u32,
    pub jitdump_path_receiver: Receiver<PathBuf>,
}

pub struct Sampler {
    command_name: String,
    task_receiver: Receiver<TaskInit>,
    interval: Duration,
    time_limit: Option<Duration>,
}

impl Sampler {
    pub fn new(
        command: String,
        task_receiver: Receiver<TaskInit>,
        interval: Duration,
        time_limit: Option<Duration>,
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
            interval,
            time_limit,
        }
    }

    pub fn run(self) -> Result<Profile, SamplingError> {
        let reference_mono = get_monotonic_timestamp();
        let reference_system_time = SystemTime::now();

        let timestamp_converter = TimestampConverter::with_reference_timestamp(reference_mono);

        let mut profile = Profile::new(
            &self.command_name,
            ReferenceTimestamp::from_system_time(reference_system_time),
            self.interval.into(),
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

        let root_task = TaskProfiler::new(
            root_task_init.task,
            root_task_init.pid,
            root_task_init.jitdump_path_receiver,
            timestamp_converter.convert_time(root_task_init.start_time),
            &self.command_name,
            &mut profile,
        )
        .expect("couldn't create root TaskProfiler");

        let mut process_sample_datas = Vec::new();
        let mut stack_scratch_buffer = Vec::new();
        let mut live_tasks = vec![root_task];
        let mut unwinder_cache = Default::default();
        let mut unresolved_stacks = UnresolvedStacks::default();
        let mut last_sleep_overshoot = 0;

        loop {
            // Poll to see if there are any new tasks we should add. If no new tasks are available,
            // this completes immediately.
            while let Ok(task_init) = self.task_receiver.try_recv() {
                let new_task = match TaskProfiler::new(
                    task_init.task,
                    task_init.pid,
                    task_init.jitdump_path_receiver,
                    timestamp_converter.convert_time(task_init.start_time),
                    &self.command_name,
                    &mut profile,
                ) {
                    Ok(new_task) => new_task,
                    Err(_) => {
                        // The task is probably already dead again. We get here for tasks which are
                        // very short-lived.
                        continue;
                    }
                };

                live_tasks.push(new_task);
            }

            let sample_mono = get_monotonic_timestamp();
            if let Some(time_limit) = self.time_limit {
                if sample_mono - reference_mono >= time_limit.as_nanos() as u64 {
                    break;
                }
            }

            let sample_timestamp = timestamp_converter.convert_time(sample_mono);

            let mut tasks = Vec::with_capacity(live_tasks.capacity());
            mem::swap(&mut live_tasks, &mut tasks);
            for mut task in tasks.into_iter() {
                task.check_jitdump(
                    &mut profile,
                    &mut jit_category_manager,
                    &timestamp_converter,
                );
                let still_alive = task.sample(
                    sample_timestamp,
                    sample_mono,
                    &mut unwinder_cache,
                    &mut profile,
                    &mut stack_scratch_buffer,
                    &mut unresolved_stacks,
                )?;
                if still_alive {
                    live_tasks.push(task);
                } else {
                    task.notify_dead(sample_timestamp, &mut profile);
                    process_sample_datas.push(task.finish(
                        &mut jit_category_manager,
                        &mut profile,
                        &timestamp_converter,
                    ));
                }
            }

            if live_tasks.is_empty() {
                // All tasks we know about are dead.
                // Wait for a little more in case one of the just-ended tasks spawned a new task.
                if let Ok(task_init) = self
                    .task_receiver
                    .recv_timeout(Duration::from_secs_f32(0.5))
                {
                    // Got one!
                    let new_task = TaskProfiler::new(
                        task_init.task,
                        task_init.pid,
                        task_init.jitdump_path_receiver,
                        timestamp_converter.convert_time(task_init.start_time),
                        &self.command_name,
                        &mut profile,
                    )
                    .expect("couldn't create TaskProfiler");
                    live_tasks.push(new_task);
                } else {
                    eprintln!("All tasks terminated.");
                    break;
                }
            }

            let intended_wakeup_time = sample_mono + self.interval.as_nanos() as u64;
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
            process_sample_datas.push(task.finish(
                &mut jit_category_manager,
                &mut profile,
                &timestamp_converter,
            ));
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
