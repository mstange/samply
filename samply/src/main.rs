use clap::{Args, Parser, Subcommand};
use samply_server::PortSelection;

use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod mac;

mod server;

// To avoid warnings about unused declarations
#[cfg(target_os = "macos")]
pub use mac::{kernel_error, thread_act, thread_info};

use server::{start_server_main, ServerProps};

#[derive(Debug, Parser)]
#[clap(
    name = "samply",
    about = r#"
samply is a sampling CPU profiler.
Run a command, record a CPU profile of its execution, and open the profiler UI.
On non-macOS platforms, samply can only load existing profiles.

EXAMPLES:
    # Default usage:
    samply record ./yourcommand yourargs

    # Alternative usage: Save profile to file for later viewing, and then load it.
    samply record --save-only -o prof.json -- ./yourcommand yourargs
    samply load prof.json
"#
)]
struct Opt {
    #[clap(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Load a profile from a file and display it.
    Load(LoadArgs),

    #[cfg(target_os = "macos")]
    /// Record a profile and display it.
    Record(RecordArgs),
}

#[derive(Debug, Args)]
struct LoadArgs {
    /// Path to the file that should be loaded.
    #[clap(parse(from_os_str))]
    file: PathBuf,

    #[clap(flatten)]
    server_args: ServerArgs,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Args)]
#[clap(trailing_var_arg = true)]
struct RecordArgs {
    /// Do not run a local server after recording.
    #[clap(short, long)]
    save_only: bool,

    /// Sampling rate, in Hz
    #[clap(short, long, default_value = "1000")]
    rate: f64,

    /// Limit the recorded time to the specified number of seconds
    #[clap(short, long)]
    duration: Option<f64>,

    /// Output filename.
    #[clap(short, long, default_value = "profile.json", parse(from_os_str))]
    output: PathBuf,

    #[clap(flatten)]
    server_args: ServerArgs,

    /// Profile the execution of this command.
    #[clap(required = true)]
    command: std::ffi::OsString,

    /// The arguments passed to the recorded command.
    #[clap(multiple_values = true, allow_hyphen_values = true)]
    command_args: Vec<std::ffi::OsString>,
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Do not open the profiler UI.
    #[clap(short, long)]
    no_open: bool,

    /// The port to use for the local web server
    #[clap(short, long, default_value = "3000+")]
    port: String,

    /// Print debugging output.
    #[clap(short, long)]
    verbose: bool,
}

fn main() {
    let opt = Opt::from_args();
    match opt.action {
        Action::Load(load_args) => {
            start_server_main(&load_args.file, load_args.server_args.server_props());
        }

        #[cfg(target_os = "macos")]
        Action::Record(record_args) => {
            use std::time::Duration;

            let server_props = if record_args.save_only {
                None
            } else {
                Some(record_args.server_args.server_props())
            };

            let time_limit = record_args.duration.map(Duration::from_secs_f64);
            if record_args.rate <= 0.0 {
                eprintln!(
                    "Error: sampling rate must be greater than zero, got {}",
                    record_args.rate
                );
                std::process::exit(1);
            }
            let interval = Duration::from_secs_f64(1.0 / record_args.rate);
            let exit_status = match mac::profiler::start_recording(
                &record_args.output,
                record_args.command,
                &record_args.command_args,
                time_limit,
                interval,
                server_props,
            ) {
                Ok(exit_status) => exit_status,
                Err(err) => {
                    eprintln!("Encountered a mach error during profiling: {:?}", err);
                    std::process::exit(1);
                }
            };
            std::process::exit(exit_status.code().unwrap_or(0));
        }
    }
}

impl ServerArgs {
    pub fn server_props(&self) -> ServerProps {
        let open_in_browser = !self.no_open;
        let port_selection = match PortSelection::try_from_str(&self.port) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "Could not parse port as <u16> or <u16>+, got port {}, error: {}",
                    self.port, e
                );
                std::process::exit(1)
            }
        };
        ServerProps {
            port_selection,
            verbose: self.verbose,
            open_in_browser,
        }
    }
}
