use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::process::ExitStatus;
use std::thread;
use std::time::Duration;

use crossbeam_channel::unbounded;
use serde_json::to_writer;

use super::error::SamplingError;
use super::process_launcher::{
    ExistingProcessRunner, MachError, ReceivedStuff, RootTaskRunner, TaskAccepter, TaskLauncher,
};
use super::sampler::{JitdumpOrMarkerPath, Sampler, TaskInit, TaskInitOrShutdown};
use super::time::get_monotonic_timestamp;
use crate::server::{start_server_main, ServerProps};
use crate::shared::recording_props::{
    ProcessLaunchProps, ProfileCreationProps, RecordingMode, RecordingProps,
};

pub fn start_recording(
    recording_mode: RecordingMode,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, MachError> {
    let mut unlink_aux_files = profile_creation_props.unlink_aux_files;
    let output_file = recording_props.output_file.clone();
    let profile_name;

    let mut task_accepter = TaskAccepter::new()?;

    let root_task_runner: Box<dyn RootTaskRunner> = match recording_mode {
        RecordingMode::All => {
            // TODO: Implement, by sudo launching a helper process which uses task_for_pid
            eprintln!("Error: Profiling existing processes is currently not supported on macOS.");
            eprintln!("You can only profile processes which you launch via samply.");
            std::process::exit(1)
        }
        RecordingMode::Pid(pid) => {
            profile_name = format!("pid {pid}");

            Box::new(ExistingProcessRunner::new(pid, &mut task_accepter))
        }
        RecordingMode::Launch(process_launch_props) => {
            profile_name = process_launch_props
                .command_name
                .to_string_lossy()
                .to_string();

            let ProcessLaunchProps {
                mut env_vars,
                command_name,
                args,
                iteration_count,
            } = process_launch_props;

            if recording_props.coreclr {
                // We need to set DOTNET_PerfMapEnabled=2 in the environment if it's not already set.
                // If we set it, we'll also set unlink_aux_files=true to avoid leaving files
                // behind in the temp directory. But if it's set manually, assume the user
                // knows what they're doing and will specify the arg as needed.
                if !env_vars.iter().any(|p| p.0 == "DOTNET_PerfMapEnabled") {
                    env_vars.push(("DOTNET_PerfMapEnabled".into(), "2".into()));
                    unlink_aux_files = true;
                }
            }

            let task_launcher = TaskLauncher::new(
                &command_name,
                &args,
                iteration_count,
                &env_vars,
                task_accepter.extra_env_vars(),
            )?;

            Box::new(task_launcher)
        }
    };

    let profile_creation_props = ProfileCreationProps {
        unlink_aux_files,
        ..profile_creation_props
    };

    let (task_sender, task_receiver) = unbounded();

    let sampler_thread = thread::spawn(move || {
        let sampler = Sampler::new(
            profile_name,
            task_receiver,
            recording_props,
            profile_creation_props,
        );
        sampler.run()
    });

    let (accepter_sender, accepter_receiver) = unbounded();
    let accepter_thread = thread::spawn(move || {
        // Loop while accepting messages from the spawned process tree.

        // A map of pids to channel senders, to notify existing tasks of Jitdump
        // paths. Having the mapping here lets us deliver the path to the right
        // task even in cases where a process execs into a new task with the same pid.
        let mut path_senders_per_pid = HashMap::new();

        loop {
            if let Ok(()) = accepter_receiver.try_recv() {
                task_sender.send(TaskInitOrShutdown::Shutdown).ok();
                break;
            }
            let timeout = Duration::from_secs_f64(1.0);
            match task_accepter.next_message(timeout) {
                Ok(ReceivedStuff::AcceptedTask(accepted_task)) => {
                    let pid = accepted_task.get_id();
                    let (path_sender, path_receiver) = unbounded();
                    let send_result = task_sender.send(TaskInitOrShutdown::TaskInit(TaskInit {
                        start_time_mono: get_monotonic_timestamp(),
                        task: accepted_task.task(),
                        pid,
                        path_receiver,
                    }));
                    path_senders_per_pid.insert(pid, path_sender);
                    if send_result.is_err() {
                        // The sampler has already shut down. This task arrived too late.
                    }
                    accepted_task.start_execution();
                }
                Ok(ReceivedStuff::JitdumpPath(pid, path)) => {
                    match path_senders_per_pid.entry(pid) {
                        Entry::Occupied(mut entry) => {
                            let send_result =
                                entry.get_mut().send(JitdumpOrMarkerPath::JitdumpPath(path));
                            if send_result.is_err() {
                                // The task is probably already dead. The path arrived too late.
                                entry.remove();
                            }
                        }
                        Entry::Vacant(_entry) => {
                            eprintln!(
                                "Received a Jitdump path for pid {pid} which I don't have a task for."
                            );
                        }
                    }
                }
                Ok(ReceivedStuff::MarkerFilePath(pid, path)) => {
                    match path_senders_per_pid.entry(pid) {
                        Entry::Occupied(mut entry) => {
                            let send_result = entry
                                .get_mut()
                                .send(JitdumpOrMarkerPath::MarkerFilePath(path));
                            if send_result.is_err() {
                                // The task is probably already dead. The path arrived too late.
                                entry.remove();
                            }
                        }
                        Entry::Vacant(_entry) => {
                            eprintln!(
                                "Received a marker file path for pid {pid} which I don't have a task for."
                            );
                        }
                    }
                }
                Err(MachError::RcvTimedOut) => {
                    // TODO: give status back via task_sender
                }
                Err(err) => {
                    eprintln!("Encountered error while waiting for task port: {err:?}");
                }
            }
        }
    });

    // Run the root task: either launch or attach to existing pid
    let exit_status = root_task_runner.run_root_task()?;

    accepter_sender
        .send(())
        .expect("couldn't tell accepter thread to stop");
    accepter_thread
        .join()
        .expect("couldn't join accepter thread");

    // Wait for the sampler to stop. It will run until all accepted tasks have terminated,
    // or until the time limit has elapsed.
    let profile_result = sampler_thread.join().expect("couldn't join sampler thread");

    let profile = match profile_result {
        Ok(profile) => profile,
        Err(SamplingError::CouldNotObtainRootTask) => {
            eprintln!("Profiling failed: Could not obtain the root task.");
            eprintln!();
            eprintln!("On macOS, samply cannot profile system commands, such as the sleep command or system python. This is because system executables are signed in such a way that they block the DYLD_INSERT_LIBRARIES environment variable, which subverts samply's attempt to siphon out the mach task port of the process.");
            eprintln!();
            eprintln!("Suggested remedy: You can profile any binaries that you've compiled yourself, or which are unsigned or locally-signed, such as anything installed by cargo install or by Homebrew.");
            std::process::exit(1)
        }
        Err(e) => {
            eprintln!("An error occurred during profiling: {e}");
            std::process::exit(1)
        }
    };

    {
        // Write the profile to a file.
        let file = File::create(&output_file).unwrap();
        let writer = BufWriter::new(file);
        to_writer(writer, &profile).expect("Couldn't write JSON");
    }

    if let Some(server_props) = server_props {
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(&output_file).expect("Couldn't open file we just wrote"),
            &output_file,
        )
        .expect("Couldn't parse libinfo map from profile file");

        start_server_main(&output_file, server_props, libinfo_map);
    }

    Ok(exit_status)
}
