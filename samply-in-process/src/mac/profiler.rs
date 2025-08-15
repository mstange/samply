use std::thread::{self, JoinHandle};

use crossbeam_channel::unbounded;
use fxprof_processed_profile::Profile;
use mach2::traps::mach_task_self;

use super::error::SamplingError;
use super::sampler::{ProcessSpecificPath, Sampler, TaskInit, TaskInitOrShutdown};
use super::time::get_monotonic_timestamp;
use crate::shared::prop_types::{ProfileCreationProps, RecordingProps};

pub struct RunningProfiler {
    task_sender: crossbeam_channel::Sender<TaskInitOrShutdown>,
    sampler_thread: JoinHandle<Result<Profile, SamplingError>>,
    _path_sender: crossbeam_channel::Sender<ProcessSpecificPath>,
}

impl RunningProfiler {
    pub fn start_recording(
        recording_props: RecordingProps,
        profile_creation_props: ProfileCreationProps,
    ) -> Self {
        let (task_sender, task_receiver) = unbounded();

        let sampler_thread = thread::spawn(move || {
            let sampler = Sampler::new(task_receiver, recording_props, profile_creation_props);
            sampler.run()
        });

        let self_task = unsafe { mach_task_self() };
        let pid = std::process::id();
        let (path_sender, path_receiver) = unbounded();

        task_sender
            .send(TaskInitOrShutdown::TaskInit(TaskInit {
                start_time_mono: get_monotonic_timestamp(),
                task: self_task,
                pid,
                path_receiver,
            }))
            .unwrap();

        Self {
            task_sender,
            sampler_thread,
            _path_sender: path_sender,
        }
    }

    pub fn stop_and_capture_profile(self) -> Result<Profile, SamplingError> {
        self.task_sender.send(TaskInitOrShutdown::Shutdown).unwrap();
        self.sampler_thread
            .join()
            .expect("couldn't join sampler thread")
    }
}
