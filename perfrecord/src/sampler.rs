use super::gecko_profile::ProfileBuilder;
use super::kernel_error;
use super::task_profiler::TaskProfiler;
use crossbeam_channel::Receiver;
use std::mem;
use std::thread;
use std::time::{Duration, Instant};

pub struct Sampler {
    task_receiver: Receiver<TaskProfiler>,
    sampling_start: Instant,
    interval: Duration,
    time_limit: Option<Duration>,
    live_root_task: Option<TaskProfiler>,
    live_other_tasks: Vec<TaskProfiler>,
    dead_root_task: Option<TaskProfiler>,
    dead_other_tasks: Vec<TaskProfiler>,
}

impl Sampler {
    pub fn new(
        task_receiver: Receiver<TaskProfiler>,
        interval: Duration,
        time_limit: Option<Duration>,
    ) -> Self {
        Sampler {
            task_receiver,
            sampling_start: Instant::now(),
            interval,
            time_limit,
            live_root_task: None,
            live_other_tasks: Vec::new(),
            dead_root_task: None,
            dead_other_tasks: Vec::new(),
        }
    }

    pub fn run(mut self) -> kernel_error::Result<ProfileBuilder> {
        let root_task = match self.task_receiver.recv() {
            Ok(task) => task,
            Err(_) => {
                // The sender went away. No profiling today.
                eprintln!("The process we launched did not give us a task port. This commonly happens when trying to profile signed executables (system apps, system python, ...), because those ignore DYLD_INSERT_LIBRARIES (and stop it from inheriting into child processes). For now, this profiler can only be used on unsigned binaries.");
                return Err(kernel_error::KernelError::MachRcvPortDied);
            }
        };

        self.live_root_task = Some(root_task);

        let mut last_sleep_overshoot = Duration::from_nanos(0);

        loop {
            while let Ok(new_task) = self.task_receiver.try_recv() {
                self.live_other_tasks.push(new_task);
            }

            let sample_timestamp = Instant::now();
            if let Some(time_limit) = self.time_limit {
                if sample_timestamp.duration_since(self.sampling_start) >= time_limit {
                    break;
                }
            }

            if let Some(task) = &mut self.live_root_task {
                let still_alive = task.sample(sample_timestamp)?;
                if !still_alive {
                    task.notify_dead(sample_timestamp);
                    self.dead_root_task = self.live_root_task.take();
                }
            }

            let mut other_tasks = Vec::with_capacity(self.live_other_tasks.capacity());
            mem::swap(&mut self.live_other_tasks, &mut other_tasks);
            for mut task in other_tasks.into_iter() {
                let still_alive = task.sample(sample_timestamp)?;
                if still_alive {
                    self.live_other_tasks.push(task);
                } else {
                    task.notify_dead(sample_timestamp);
                    self.dead_other_tasks.push(task);
                }
            }

            if self.live_root_task.is_none() && self.live_other_tasks.is_empty() {
                // All tasks we know about are dead.
                // Wait for a little more in case one of the just-ended tasks spawned a new task.
                if let Ok(new_task) = self
                    .task_receiver
                    .recv_timeout(Duration::from_secs_f32(0.5))
                {
                    // Got one!
                    self.live_other_tasks.push(new_task);
                } else {
                    println!("All tasks terminated.");
                    break;
                }
            }

            let intended_wakeup_time = sample_timestamp + self.interval;
            let indended_wait_time = intended_wakeup_time.saturating_duration_since(Instant::now());
            let sleep_time = if indended_wait_time > last_sleep_overshoot {
                indended_wait_time - last_sleep_overshoot
            } else {
                Duration::from_nanos(0)
            };
            sleep_and_save_overshoot(sleep_time, &mut last_sleep_overshoot);
        }

        let root_task = self.live_root_task.or(self.dead_root_task).unwrap();
        let other_tasks: Vec<_> = self
            .live_other_tasks
            .into_iter()
            .chain(self.dead_other_tasks.into_iter())
            .collect();

        Ok(root_task.into_profile(other_tasks))
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
