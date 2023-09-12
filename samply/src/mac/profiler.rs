use crossbeam_channel::unbounded;
use serde_json::to_writer;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::BufWriter;
use std::process::ExitStatus;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::error::SamplingError;
use super::process_launcher::{MachError, ReceivedStuff, TaskAccepter};
use super::sampler::{Sampler, TaskInit};
use super::time::get_monotonic_timestamp;
use crate::server::{start_server_main, ServerProps};
use crate::shared::recording_props::{ConversionProps, RecordingProps};

pub fn start_profiling_pid(
    _pid: u32,
    _recording_props: RecordingProps,
    _conversion_props: ConversionProps,
    _server_props: Option<ServerProps>,
) {
    eprintln!("Profiling existing processes is currently not supported on macOS.");
    eprintln!("You can only profile processes which you launch via samply.");
    std::process::exit(1)
}

pub fn start_recording(
    command_name: OsString,
    command_args: &[OsString],
    iteration_count: u32,
    recording_props: RecordingProps,
    conversion_props: ConversionProps,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, MachError> {
    let (task_sender, task_receiver) = unbounded();
    let command_name_copy = command_name.to_string_lossy().to_string();
    let output_file = recording_props.output_file.clone();
    let sampler_thread = thread::spawn(move || {
        let sampler = Sampler::new(
            command_name_copy,
            task_receiver,
            recording_props,
            conversion_props,
        );
        sampler.run()
    });

    // Ignore SIGINT while the subcommand is running. The signal still reaches the process
    // under observation while we continue to record it. (ctrl+c will send the SIGINT signal
    // to all processes in the foreground process group).
    let should_terminate_on_ctrl_c = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    signal_hook::flag::register_conditional_default(
        signal_hook::consts::SIGINT,
        should_terminate_on_ctrl_c.clone(),
    )
    .expect("cannot register signal handler");

    let (mut task_accepter, task_launcher) = TaskAccepter::new(&command_name, command_args)?;

    let (accepter_sender, accepter_receiver) = unbounded();
    let accepter_thread = thread::spawn(move || {
        // Loop while accepting messages from the spawned process tree.

        // A map of pids to channel senders, to notify existing tasks of Jitdump
        // paths. Having the mapping here lets us deliver the path to the right
        // task even in cases where a process execs into a new task with the same pid.
        let mut jitdump_path_senders_per_pid = HashMap::new();

        loop {
            if let Ok(()) = accepter_receiver.try_recv() {
                break;
            }
            let timeout = Duration::from_secs_f64(1.0);
            match task_accepter.next_message(timeout) {
                Ok(ReceivedStuff::AcceptedTask(mut accepted_task)) => {
                    let pid = accepted_task.get_id();
                    let (jitdump_path_sender, jitdump_path_receiver) = unbounded();
                    let send_result = task_sender.send(TaskInit {
                        start_time_mono: get_monotonic_timestamp(),
                        task: accepted_task.take_task(),
                        pid,
                        jitdump_path_receiver,
                    });
                    jitdump_path_senders_per_pid.insert(pid, jitdump_path_sender);
                    if send_result.is_err() {
                        // The sampler has already shut down. This task arrived too late.
                    }
                    accepted_task.start_execution();
                }
                Ok(ReceivedStuff::JitdumpPath(pid, path)) => {
                    match jitdump_path_senders_per_pid.entry(pid) {
                        Entry::Occupied(mut entry) => {
                            let send_result = entry.get_mut().send(path);
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
                Err(MachError::RcvTimedOut) => {
                    // TODO: give status back via task_sender
                }
                Err(err) => {
                    eprintln!("Encountered error while waiting for task port: {err:?}");
                }
            }
        }
    });

    let mut root_child = task_launcher.launch_child();
    let mut exit_status = root_child.wait().expect("couldn't wait for child");

    for i in 2..=iteration_count {
        if !exit_status.success() {
            eprintln!(
                "Skipping remaining iterations due to non-success exit status: \"{}\"",
                exit_status
            );
            break;
        }
        eprintln!("Running iteration {i} of {iteration_count}...");
        let mut root_child = task_launcher.launch_child();
        exit_status = root_child.wait().expect("couldn't wait for child");
    }

    // The launched subprocess is done. From now on, we want to terminate if the user presses Ctrl+C.
    should_terminate_on_ctrl_c.store(true, std::sync::atomic::Ordering::SeqCst);

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

    let file = File::create(&output_file).unwrap();
    let writer = BufWriter::new(file);
    to_writer(writer, &profile).expect("Couldn't write JSON");

    if let Some(server_props) = server_props {
        start_server_main(&output_file, server_props);
    }

    Ok(exit_status)
}
