use samply_server::PortSelection;
use structopt::StructOpt;

use std::path::PathBuf;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod mac;

// To avoid warnings about unused declarations
#[cfg(target_os = "macos")]
pub use mac::{kernel_error, thread_act, thread_info};

mod server;

use server::{start_server_main, ServerProps};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "samply",
    about = r#"
samply is a sampling CPU profiler.
Run a command, record a CPU profile of its execution, and open the profiler UI.

EXAMPLES:
    # Default usage:
    samply ./yourcommand yourargs

    # Alternative usage: Save profile to file for later viewing, and then load it.
    samply --save-only -o prof.json ./yourcommand yourargs
    samply --load prof.json
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

fn main() {
    let opt = Opt::from_args();
    if let Some(file) = opt.load.as_deref() {
        start_server_main(file, opt.server_props());
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let server_props = if opt.save_only {
            None
        } else {
            Some(opt.server_props())
        };

        let command = match opt.rest {
            Some(Subcommands::Command(command)) if !command.is_empty() => command,
            _ => {
                eprintln!("Error: missing command\n");
                Opt::clap().print_help().unwrap();
                println!();
                std::process::exit(1);
            }
        };

        let time_limit = opt.time_limit.map(Duration::from_secs_f64);
        if opt.frequency <= 0.0 {
            eprintln!(
                "Error: sampling frequency must be greater than zero, got {}",
                opt.frequency
            );
            std::process::exit(1);
        }
        let interval = Duration::from_secs_f64(1.0 / opt.frequency);
        let exit_status = match mac::profiler::start_recording(
            &opt.output_file,
            &command,
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

impl Opt {
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
