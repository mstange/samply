use crossbeam_channel::unbounded;
use profiler_symbol_server::start_server;
use serde_json::to_writer;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};
use structopt::StructOpt;

#[allow(deref_nullptr)]
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
    about = r#"
Run a command, record a CPU profile of its execution, and open the profiler.

By default, perfrecord launches the profiler at the end of execution.

EXAMPLES:
    perfrecord ./yourcommand yourargs
    perfrecord --save-only -o prof.json ./yourcommand yourargs
    perfrecord --load prof.json
"#
)]
struct Opt {
    /// Do not open the profiler after recording, only run the server.
    #[structopt(short, long = "no-open")]
    no_open: bool,

    /// Only save the recorded profile to a file, and then exit.
    /// The file can be opened in the profiler using perfrecord --load filename.
    #[structopt(short, long = "save-only")]
    save_only: bool,

    /// Sampling interval, in seconds
    #[structopt(short = "i", long = "interval", default_value = "0.001")]
    interval: f64,

    /// Limit the recorded time to the specified number of seconds
    #[structopt(short = "t", long = "time-limit")]
    time_limit: Option<f64>,

    /// The port to use for the local web server
    #[structopt(short, long, default_value = "3000")]
    port: u16,

    /// Save the collected profile to this file.
    #[structopt(
        short = "o",
        long = "out",
        default_value = "profile.json",
        parse(from_os_str)
    )]
    output_file: PathBuf,

    /// Profile the execution of this command. Ignored if --load is specified.
    #[structopt(subcommand)]
    rest: Option<Subcommands>,

    /// Don't record. Instead, load the specified file in the profiler, using your default browser.
    #[structopt(short = "l", long = "load", parse(from_os_str))]
    load: Option<PathBuf>,
}

#[derive(Debug, PartialEq, StructOpt)]
enum Subcommands {
    #[structopt(external_subcommand)]
    Command(Vec<String>),
}

fn main() -> Result<(), MachError> {
    let opt = Opt::from_args();
    let open_in_browser = !opt.no_open;
    if let Some(file) = opt.load {
        start_server_main(&file, opt.port, open_in_browser);
        return Ok(());
    }
    if let Some(Subcommands::Command(command)) = opt.rest {
        if !command.is_empty() {
            let time_limit = opt.time_limit.map(Duration::from_secs_f64);
            let interval = Duration::from_secs_f64(opt.interval);
            let exit_status = start_recording(
                &opt.output_file,
                opt.port,
                &command,
                time_limit,
                interval,
                !opt.save_only,
                open_in_browser,
            )?;
            std::process::exit(exit_status.code().unwrap_or(0));
        }
    }
    println!("Error: missing command\n");
    Opt::clap().print_help().unwrap();
    std::process::exit(1);
}

#[tokio::main]
async fn start_server_main(file: &Path, port: u16, open_in_browser: bool) {
    start_server(file, port, open_in_browser).await;
}

fn start_recording(
    output_file: &Path,
    port: u16,
    args: &[String],
    time_limit: Option<Duration>,
    interval: Duration,
    serve_when_done: bool,
    open_in_browser: bool,
) -> Result<ExitStatus, MachError> {
    let (saver_sender, saver_receiver) = unbounded();
    let output_file = output_file.to_owned();
    let saver_thread = thread::spawn(move || {
        let profile_builder: ProfileBuilder = saver_receiver.recv().expect("saver couldn't recv");
        let file = File::create(&output_file).unwrap();
        to_writer(file, &profile_builder.to_json()).expect("Couldn't write JSON");

        // Reuse the saver thread as the server thread.
        if serve_when_done {
            start_server_main(&output_file, port, open_in_browser);
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
            if send_result.is_err() {
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
