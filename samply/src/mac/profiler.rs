use crossbeam_channel::unbounded;
use fxprof_processed_profile::Profile;
use serde_json::to_writer;

use std::ffi::OsString;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::process_launcher::{MachError, TaskAccepter};
use super::sampler::{Sampler, TaskInit};
use crate::server::{start_server_main, ServerProps};

pub fn start_profiling_pid(
    _output_file: &Path,
    _pid: u32,
    _time_limit: Option<Duration>,
    _interval: Duration,
    _server_props: Option<ServerProps>,
) {
    eprintln!("Profiling existing processes is currently not supported on macOS.");
    eprintln!("You can only profile processes which you launch via samply.");
    std::process::exit(-1)
}

pub fn start_recording(
    output_file: &Path,
    command_name: OsString,
    command_args: &[OsString],
    time_limit: Option<Duration>,
    interval: Duration,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, MachError> {
    let (saver_sender, saver_receiver) = unbounded();
    let output_file = output_file.to_owned();
    let saver_thread = thread::spawn(move || {
        let profile: Profile = saver_receiver.recv().expect("saver couldn't recv");
        let file = File::create(&output_file).unwrap();
        let writer = BufWriter::new(file);
        to_writer(writer, &profile).expect("Couldn't write JSON");

        // Reuse the saver thread as the server thread.
        if let Some(server_props) = server_props {
            start_server_main(&output_file, server_props);
        }
    });

    let (task_sender, task_receiver) = unbounded();
    let command_name_copy = command_name.to_string_lossy().to_string();
    let sampler_thread = thread::spawn(move || {
        let sampler = Sampler::new(command_name_copy, task_receiver, interval, time_limit);
        let profile = sampler.run().expect("Sampler ran into an error");
        saver_sender.send(profile).expect("couldn't send profile");
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

    let (mut task_accepter, mut root_child) =
        TaskAccepter::create_and_launch_root_task(&command_name, command_args)?;

    let (accepter_sender, accepter_receiver) = unbounded();
    let accepter_thread = thread::spawn(move || loop {
        if let Ok(()) = accepter_receiver.try_recv() {
            break;
        }
        let timeout = Duration::from_secs_f64(1.0);
        match task_accepter.try_accept(timeout) {
            Ok(mut accepted_task) => {
                let send_result = task_sender.send(TaskInit {
                    start_time: Instant::now(),
                    task: accepted_task.take_task(),
                    pid: accepted_task.get_id(),
                });
                if send_result.is_err() {
                    // The sampler has already shut down. This task arrived too late.
                }
                accepted_task.start_execution();
            }
            Err(MachError::RcvTimedOut) => {
                // TODO: give status back via task_sender
            }
            Err(err) => {
                eprintln!("Encountered error while waiting for task port: {:?}", err);
            }
        }
    });

    let exit_status = root_child.wait().expect("couldn't wait for child");

    // The subprocess is done. From now on, we want to terminate if the user presses Ctrl+C.
    should_terminate_on_ctrl_c.store(true, std::sync::atomic::Ordering::SeqCst);

    accepter_sender
        .send(())
        .expect("couldn't tell accepter thread to stop");
    accepter_thread
        .join()
        .expect("couldn't join accepter thread");
    sampler_thread.join().expect("couldn't join sampler thread");
    saver_thread.join().expect("couldn't join saver thread");

    Ok(exit_status)
}
