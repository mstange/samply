use crossbeam_channel::unbounded;
use profiler_symbol_server::start_server;
use serde_json::to_writer;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};
use structopt::StructOpt;

mod dyld_bindings;
mod gecko_profile;
mod proc_maps;
mod process_launcher;
mod sampler;
mod task_profiler;
mod thread_profiler;

pub mod kernel_error;
pub mod thread_act;
pub mod thread_info;

use gecko_profile::ProfileBuilder;
use process_launcher::{MachError, TaskAccepter};
use sampler::Sampler;
use task_profiler::TaskProfiler;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "perfrecord",
    about = r#"Run a command and record a CPU profile of its execution.

EXAMPLES:
    perfrecord ./yourcommand args
    perfrecord --launch-when-done ./yourcommand args
    perfrecord -o prof.json ./yourcommand args
    perfrecord --launch prof.json"#
)]
struct Opt {
    /// Launch the profiler after recording and display the collected profile.
    #[structopt(long = "launch-when-done")]
    launch_when_done: bool,

    /// Sampling interval, in seconds
    #[structopt(short = "i", long = "interval", default_value = "0.001")]
    interval: f64,

    /// Limit the recorded time to the specified number of seconds
    #[structopt(short = "t", long = "time-limit")]
    time_limit: Option<f64>,

    /// Save the collected profile to this file.
    #[structopt(
        short = "o",
        long = "out",
        default_value = "profile.json",
        parse(from_os_str)
    )]
    output_file: PathBuf,

    /// If neither --launch nor --serve are specified, profile this command.
    #[structopt(subcommand)]
    rest: Option<Subcommands>,

    /// Don't record. Instead, launch the profiler with the selected file in your default browser.
    #[structopt(short = "l", long = "launch", parse(from_os_str))]
    file_to_launch: Option<PathBuf>,

    /// Don't record. Instead, serve the selected file from a local webserver.
    #[structopt(short = "s", long = "serve", parse(from_os_str))]
    file_to_serve: Option<PathBuf>,
}

#[derive(Debug, PartialEq, StructOpt)]
enum Subcommands {
    #[structopt(external_subcommand)]
    Command(Vec<String>),
}

fn main() -> Result<(), MachError> {
    let opt = Opt::from_args();
    let open_in_browser = opt.file_to_launch.is_some();
    let file_for_launching_or_serving = opt.file_to_launch.or(opt.file_to_serve);
    if let Some(file) = file_for_launching_or_serving {
        start_server_main(&file, open_in_browser);
        return Ok(());
    }
    if let Some(Subcommands::Command(command)) = opt.rest {
        if !command.is_empty() {
            let time_limit = opt.time_limit.map(|secs| Duration::from_secs_f64(secs));
            let interval = Duration::from_secs_f64(opt.interval);
            let exit_status = start_recording(
                &opt.output_file,
                &command,
                time_limit,
                interval,
                opt.launch_when_done,
            )?;
            std::process::exit(exit_status.code().unwrap_or(0));
        }
    }
    println!("Error: missing command\n");
    Opt::clap().print_help().unwrap();
    std::process::exit(1);
}

#[tokio::main]
async fn start_server_main(file: &Path, open_in_browser: bool) {
    start_server(file, open_in_browser).await;
}

fn start_recording(
    output_file: &Path,
    args: &[String],
    time_limit: Option<Duration>,
    interval: Duration,
    launch_when_done: bool,
) -> Result<ExitStatus, MachError> {
    let (saver_sender, saver_receiver) = unbounded();
    let output_file = output_file.to_owned();
    let saver_thread = thread::spawn(move || {
        let profile_builder: ProfileBuilder = saver_receiver.recv().expect("saver couldn't recv");
        let file = File::create(&output_file).unwrap();
        to_writer(file, &profile_builder.to_json()).expect("Couldn't write JSON");

        // Reuse the saver thread as the server thread.
        if launch_when_done {
            start_server_main(&output_file, true);
        }
    });

    let (task_sender, task_receiver) = unbounded();
    let sampler_thread = thread::spawn(move || {
        let sampler = Sampler::new(task_receiver, interval, time_limit);
        let profile_builder = sampler.run().expect("Sampler ran into an error");
        saver_sender
            .send(profile_builder)
            .expect("couldn't send profile");
    });

    let command_name = args.first().unwrap().clone();
    let args: Vec<&str> = args.iter().skip(1).map(std::ops::Deref::deref).collect();

    let (mut task_accepter, mut root_child) =
        TaskAccepter::create_and_launch_root_task(&command_name, &args)?;

    let (accepter_sender, accepter_receiver) = unbounded();
    let accepter_thread = thread::spawn(move || loop {
        if let Ok(()) = accepter_receiver.try_recv() {
            break;
        }
        if let Ok(mut accepted_task) = task_accepter.try_accept(Duration::from_secs_f64(0.5)) {
            let task_profiler = TaskProfiler::new(
                accepted_task.take_task(),
                accepted_task.get_id(),
                Instant::now(),
                &command_name,
                interval,
            )
            .expect("couldn't create TaskProfiler");
            let send_result = task_sender.send(task_profiler);
            if let Err(_) = send_result {
                // The sampler has already shut down. This task arrived too late.
            }
            accepted_task.start_execution();
        }
    });

    let exit_status = root_child.wait().expect("couldn't wait for child");

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
