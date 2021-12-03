use crossbeam_channel::unbounded;
use profiler_symbol_server::{get_symbol_path_from_environment, start_server, PortSelection};
use serde_json::to_writer;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use structopt::StructOpt;

#[allow(deref_nullptr)]
mod dyld_bindings;

mod error;
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
perfrecord is a sampling CPU profiler.
Run a command, record a CPU profile of its execution, and open the profiler UI.

EXAMPLES:
    # Default usage:
    perfrecord ./yourcommand yourargs

    # Alternative usage: Save profile to file for later viewing, and then load it.
    perfrecord --save-only -o prof.json ./yourcommand yourargs
    perfrecord --load prof.json
"#
)]
struct Opt {
    /// Do not open the profiler UI.
    #[structopt(short, long = "no-open")]
    no_open: bool,

    /// Do not run a local server after recording.
    #[structopt(short, long = "save-only")]
    save_only: bool,

    /// Sampling frequency, in Hz
    #[structopt(short, long, default_value = "1000")]
    frequency: f64,

    /// Limit the recorded time to the specified number of seconds
    #[structopt(short = "t", long = "time-limit")]
    time_limit: Option<f64>,

    /// The port to use for the local web server
    #[structopt(short, long, default_value = "3000+")]
    port: String,

    /// Print debugging output.
    #[structopt(short, long)]
    verbose: bool,

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

    /// Don't record. Instead, load the specified file in the profiler.
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
    let port_selection = match PortSelection::try_from_str(&opt.port) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "Could not parse port as <u16> or <u16>+, got port {}, error: {}",
                opt.port, e
            );
            std::process::exit(1)
        }
    };
    let server_props = ServerProps {
        port_selection,
        verbose: opt.verbose,
        open_in_browser,
    };
    if let Some(file) = opt.load {
        start_server_main(&file, server_props);
        return Ok(());
    }
    if let Some(Subcommands::Command(command)) = opt.rest {
        if !command.is_empty() {
            let time_limit = opt.time_limit.map(Duration::from_secs_f64);
            if opt.frequency <= 0.0 {
                eprintln!(
                    "Error: sampling frequency must be greater than zero, got {}",
                    opt.frequency
                );
                std::process::exit(1);
            }
            let interval = Duration::from_secs_f64(1.0 / opt.frequency);
            let exit_status = start_recording(
                &opt.output_file,
                &command,
                time_limit,
                interval,
                !opt.save_only,
                server_props,
            )?;
            std::process::exit(exit_status.code().unwrap_or(0));
        }
    }
    eprintln!("Error: missing command\n");
    Opt::clap().print_help().unwrap();
    println!();
    std::process::exit(1);
}

#[derive(Clone, Debug)]
struct ServerProps {
    port_selection: PortSelection,
    verbose: bool,
    open_in_browser: bool,
}

#[tokio::main]
async fn start_server_main(file: &Path, props: ServerProps) {
    start_server(
        Some(file),
        props.port_selection,
        get_symbol_path_from_environment("srv**https://msdl.microsoft.com/download/symbols"),
        props.verbose,
        props.open_in_browser,
    )
    .await;
}

fn start_recording(
    output_file: &Path,
    args: &[String],
    time_limit: Option<Duration>,
    interval: Duration,
    serve_when_done: bool,
    server_props: ServerProps,
) -> Result<ExitStatus, MachError> {
    let (saver_sender, saver_receiver) = unbounded();
    let output_file = output_file.to_owned();
    let saver_thread = thread::spawn(move || {
        let profile_builder: ProfileBuilder = saver_receiver.recv().expect("saver couldn't recv");
        let file = File::create(&output_file).unwrap();
        let writer = BufWriter::new(file);
        to_writer(writer, &profile_builder.to_json()).expect("Couldn't write JSON");

        // Reuse the saver thread as the server thread.
        if serve_when_done {
            start_server_main(&output_file, server_props);
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

    let command_name = args.first().unwrap().clone();
    let args: Vec<&str> = args.iter().skip(1).map(std::ops::Deref::deref).collect();
    let (mut task_accepter, mut root_child) =
        TaskAccepter::create_and_launch_root_task(&command_name, &args)?;

    let (accepter_sender, accepter_receiver) = unbounded();
    let accepter_thread = thread::spawn(move || loop {
        if let Ok(()) = accepter_receiver.try_recv() {
            break;
        }
        let timeout = Duration::from_secs_f64(1.0);
        match task_accepter.try_accept(timeout) {
            Ok(mut accepted_task) => {
                let task_profiler = TaskProfiler::new(
                    accepted_task.take_task(),
                    accepted_task.get_id(),
                    Instant::now(),
                    SystemTime::now(),
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
