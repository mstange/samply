#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "linux")]
mod linux;

mod import;
mod linux_shared;
mod server;
mod shared;

use clap::{Args, Parser, Subcommand};
use tempfile::NamedTempFile;

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

// To avoid warnings about unused declarations
#[cfg(target_os = "macos")]
pub use mac::{kernel_error, thread_act, thread_info};

#[cfg(target_os = "linux")]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;

use server::{start_server_main, PortSelection, ServerProps};

#[derive(Debug, Parser)]
#[command(
    name = "samply",
    version,
    about = r#"
samply is a sampling CPU profiler.
Run a command, record a CPU profile of its execution, and open the profiler UI.
Recording is currently supported on Linux and macOS.
On other platforms, samply can only load existing profiles.

EXAMPLES:
    # Default usage:
    samply record ./yourcommand yourargs

    # On Linux, you can also profile existing processes by pid:
    samply record -p 12345 # Linux only

    # Alternative usage: Save profile to file for later viewing, and then load it.
    samply record --save-only -o prof.json -- ./yourcommand yourargs
    samply load prof.json # Opens in the browser and supplies symbols

    # Import perf.data files from Linux perf:
    samply load perf.data
"#
)]
struct Opt {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Load a profile from a file and display it.
    Load(LoadArgs),

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    /// Record a profile and display it.
    Record(RecordArgs),
}

#[derive(Debug, Args)]
struct LoadArgs {
    /// Path to the file that should be loaded.
    file: PathBuf,

    #[command(flatten)]
    conversion_args: ConversionArgs,

    #[command(flatten)]
    server_args: ServerArgs,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Debug, Args)]
struct RecordArgs {
    /// Do not run a local server after recording.
    #[arg(short, long)]
    save_only: bool,

    /// Sampling rate, in Hz
    #[arg(short, long, default_value = "1000")]
    rate: f64,

    /// Limit the recorded time to the specified number of seconds
    #[arg(short, long)]
    duration: Option<f64>,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json")]
    output: PathBuf,

    /// How many times to run the profiled command.
    #[arg(long, default_value = "1")]
    iteration_count: u32,

    #[command(flatten)]
    conversion_args: ConversionArgs,

    #[command(flatten)]
    server_args: ServerArgs,

    /// Profile the execution of this command.
    #[arg(
        required_unless_present = "pid",
        conflicts_with = "pid",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    command: Vec<std::ffi::OsString>,

    /// Process ID of existing process to attach to (Linux only).
    #[arg(short, long)]
    pid: Option<u32>,
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Do not open the profiler UI.
    #[arg(short, long)]
    no_open: bool,

    /// The port to use for the local web server
    #[arg(short = 'P', long, default_value = "3000+")]
    port: String,

    /// Print debugging output.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Args, Clone)]
pub struct ConversionArgs {
    /// Merge non-overlapping threads of the same name.
    #[arg(long)]
    merge_threads: bool,

    /// Fold repeated frames at the base of the stack.
    #[arg(long)]
    fold_recursive_prefix: bool,

    /// Create markers from the spans in this file.
    #[arg(long)]
    marker_file: Option<String>,

    /// Only include samples which occurred during a marker with this name.
    #[arg(long)]
    filter_by_marker_prefix: Option<String>,
}

fn main() {
    let opt = Opt::parse();
    match opt.action {
        Action::Load(load_args) => {
            let input_file = match File::open(&load_args.file) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("Could not open file {:?}: {}", load_args.file, err);
                    std::process::exit(1)
                }
            };
            let converted_temp_file =
                attempt_conversion(&load_args.file, &input_file, &load_args.conversion_args);
            let filename = match &converted_temp_file {
                Some(temp_file) => temp_file.path(),
                None => &load_args.file,
            };
            start_server_main(filename, load_args.server_args.server_props());
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
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

            if let Some(pid) = record_args.pid {
                profiler::start_profiling_pid(
                    &record_args.output,
                    pid,
                    time_limit,
                    interval,
                    server_props,
                    &record_args.conversion_args,
                );
            } else {
                let exit_status = match profiler::start_recording(
                    &record_args.output,
                    record_args.command[0].clone(),
                    &record_args.command[1..],
                    time_limit,
                    interval,
                    server_props,
                    &record_args.conversion_args,
                    record_args.iteration_count,
                ) {
                    Ok(exit_status) => exit_status,
                    Err(err) => {
                        eprintln!("Encountered an error during profiling: {err:?}");
                        std::process::exit(1);
                    }
                };
                std::process::exit(exit_status.code().unwrap_or(0));
            }
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

fn attempt_conversion(
    filename: &Path,
    input_file: &File,
    settings: &ConversionArgs,
) -> Option<NamedTempFile> {
    let path = Path::new(filename)
        .canonicalize()
        .expect("Couldn't form absolute path");
    let reader = BufReader::new(input_file);
    let output_file = tempfile::NamedTempFile::new().ok()?;
    let profile = import::perf::convert(
        reader,
        path.parent(),
        settings.merge_threads,
        settings.fold_recursive_prefix,
        settings.marker_file.as_deref(),
        settings.filter_by_marker_prefix.as_deref(),
    )
    .ok()?;
    let writer = BufWriter::new(output_file.as_file());
    serde_json::to_writer(writer, &profile).ok()?;
    Some(output_file)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Opt::command().debug_assert();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn verify_cli_record() {
        let opt = Opt::parse_from(["samply", "record", "rustup", "show"]);
        assert!(
            matches!(opt.action, Action::Record(record_args) if record_args.command == ["rustup", "show"])
        );

        let opt = Opt::parse_from(["samply", "record", "rustup", "--no-open"]);
        assert!(
        matches!(opt.action, Action::Record(record_args) if record_args.command == ["rustup", "--no-open"]),
        "Arguments of the form --arg should be considered part of the command even if they match samply options."
    );

        let opt = Opt::parse_from(["samply", "record", "--no-open", "rustup"]);
        assert!(
            matches!(opt.action, Action::Record(record_args) if record_args.command == ["rustup"] && record_args.server_args.no_open),
            "Arguments which come before the command name should be treated as samply arguments."
        );

        // Make sure you can't pass both a pid and a command name at the same time.
        let opt_res = Opt::try_parse_from(["samply", "record", "-p", "1234", "rustup"]);
        assert!(opt_res.is_err());
    }
}
