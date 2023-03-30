use crossbeam_channel::Receiver;
use fxprof_processed_profile::{
    CategoryColor, CategoryPairHandle, Profile, ReferenceTimestamp, Timestamp,
};
use mach::port::mach_port_t;

use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use super::error::SamplingError;
use super::task_profiler::TaskProfiler;

#[derive(Debug, Clone)]
pub struct TaskInit {
    pub start_time: Instant,
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
        let reference_instant = Instant::now();
        let reference_system_time = SystemTime::now();
        let timestamp_maker = InstantTimestampMaker::new(reference_instant);

        let mut profile = Profile::new(
            &self.command_name,
            ReferenceTimestamp::from_system_time(reference_system_time),
            self.interval.into(),
        );

        let default_category =
            CategoryPairHandle::from(profile.add_category("Regular", CategoryColor::Yellow));

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
            timestamp_maker.make_ts(root_task_init.start_time),
            &self.command_name,
            &mut profile,
            default_category,
        )
        .expect("couldn't create root TaskProfiler");

        let mut live_root_task = Some(root_task);
        let mut live_other_tasks = Vec::new();
        let mut dead_other_tasks = Vec::new();
        let mut unwinder_cache = Default::default();
        let mut last_sleep_overshoot = Duration::from_nanos(0);

        let sampling_start = Instant::now();

        loop {
            // Poll to see if there are any new tasks we should add. If no new tasks are available,
            // this completes immediately.
            while let Ok(task_init) = self.task_receiver.try_recv() {
                let new_task = match TaskProfiler::new(
                    task_init.task,
                    task_init.pid,
                    task_init.jitdump_path_receiver,
                    timestamp_maker.make_ts(task_init.start_time),
                    &self.command_name,
                    &mut profile,
                    default_category,
                ) {
                    Ok(new_task) => new_task,
                    Err(_) => {
                        // The task is probably already dead again. We get here for tasks which are
                        // very short-lived.
                        continue;
                    }
                };

                live_other_tasks.push(new_task);
            }

            let sample_instant = Instant::now();
            if let Some(time_limit) = self.time_limit {
                if sample_instant.duration_since(sampling_start) >= time_limit {
                    break;
                }
            }

            let sample_timestamp = timestamp_maker.make_ts(sample_instant);

            if let Some(task) = &mut live_root_task {
                task.check_jitdump();
                let still_alive =
                    task.sample(sample_timestamp, &mut unwinder_cache, &mut profile)?;
                if !still_alive {
                    task.notify_dead(sample_timestamp, &mut profile);
                    live_root_task = None;
                }
            }

            let mut other_tasks = Vec::with_capacity(live_other_tasks.capacity());
            mem::swap(&mut live_other_tasks, &mut other_tasks);
            for mut task in other_tasks.into_iter() {
                task.check_jitdump();
                let still_alive =
                    task.sample(sample_timestamp, &mut unwinder_cache, &mut profile)?;
                if still_alive {
                    live_other_tasks.push(task);
                } else {
                    task.notify_dead(sample_timestamp, &mut profile);
                    dead_other_tasks.push(task);
                }
            }

            if live_root_task.is_none() && live_other_tasks.is_empty() {
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
                        timestamp_maker.make_ts(task_init.start_time),
                        &self.command_name,
                        &mut profile,
                        default_category,
                    )
                    .expect("couldn't create TaskProfiler");
                    live_other_tasks.push(new_task);
                } else {
                    eprintln!("All tasks terminated.");
                    break;
                }
            }

            let intended_wakeup_time = sample_instant + self.interval;
            let indended_wait_time = intended_wakeup_time.saturating_duration_since(Instant::now());
            let sleep_time = if indended_wait_time > last_sleep_overshoot {
                indended_wait_time - last_sleep_overshoot
            } else {
                Duration::from_nanos(0)
            };
            sleep_and_save_overshoot(sleep_time, &mut last_sleep_overshoot);
        }

        Ok(profile)
    }
}

fn sleep_and_save_overshoot(duration: Duration, overshoot: &mut Duration) {
    let before_sleep = Instant::now();
    thread::sleep(duration);
    let after_sleep = Instant::now();
    *overshoot = after_sleep
        .duration_since(before_sleep)
        .checked_sub(duration)
        .unwrap_or_else(|| Duration::from_nanos(0));
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct InstantTimestampMaker {
    reference_instant: Instant,
}

impl InstantTimestampMaker {
    fn new(instant: Instant) -> Self {
        Self {
            reference_instant: instant,
        }
    }
}

impl InstantTimestampMaker {
    pub fn make_ts(&self, instant: Instant) -> Timestamp {
        Timestamp::from_nanos_since_reference(
            instant
                .saturating_duration_since(self.reference_instant)
                .as_nanos() as u64,
        )
    }
}
