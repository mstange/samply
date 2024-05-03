#[cfg(target_os = "macos")]
mod mac;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

mod import;
mod linux_shared;
mod profile_json_preparse;
mod server;
mod shared;

use clap::{Args, Parser, Subcommand};
use profile_json_preparse::parse_libinfo_map_from_profile_file;
use shared::recording_props::{ProcessLaunchProps, ProfileCreationProps, RecordingProps};

use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Duration;

// To avoid warnings about unused declarations
#[cfg(target_os = "macos")]
pub use mac::{kernel_error, thread_act, thread_info};

#[cfg(any(target_os = "android", target_os = "linux"))]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;
#[cfg(target_os = "windows")]
use windows::profiler;

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
    samply import perf.data
"#
)]
struct Opt {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    #[cfg(any(
        target_os = "android",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    ))]
    /// Record a profile and display it.
    Record(RecordArgs),

    /// Load a profile from a file and display it.
    Load(LoadArgs),

    /// Import a perf.data file and display the profile.
    Import(ImportArgs),
}

#[derive(Debug, Args)]
struct LoadArgs {
    /// Path to the file that should be loaded.
    file: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// Path to the profile file that should be imported.
    file: PathBuf,

    #[command(flatten)]
    profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json")]
    output: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,
}

#[allow(unused)]
#[derive(Debug, Args)]
struct RecordArgs {
    /// Sampling rate, in Hz
    #[arg(short, long, default_value = "1000")]
    rate: f64,

    /// Limit the recorded time to the specified number of seconds
    #[arg(short, long)]
    duration: Option<f64>,

    /// How many times to run the profiled command.
    #[arg(long, default_value = "1")]
    iteration_count: u32,

    /// Reduce profiling overhead by only recording the main thread.
    /// This option is only respected on macOS.
    #[arg(long)]
    main_thread_only: bool,

    #[command(flatten)]
    profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json")]
    output: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,

    /// Profile the execution of this command.
    #[arg(
        required_unless_present_any = ["pid", "all"],
        conflicts_with_all = ["pid", "all"],
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    command: Vec<std::ffi::OsString>,

    /// Process ID of existing process to attach to (Linux only).
    #[arg(short, long, conflicts_with = "all")]
    pid: Option<u32>,

    /// Profile entire system (all processes). Not supported on macOS.
    #[arg(short, long, conflicts_with = "all")]
    all: Option<u32>,
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
pub struct ProfileCreationArgs {
    /// Set a custom name for the recorded profile.
    /// By default it is either the command that was run or the process pid.
    #[arg(long)]
    profile_name: Option<String>,

    /// Merge non-overlapping threads of the same name.
    #[arg(long)]
    reuse_threads: bool,

    /// Fold repeated frames at the base of the stack.
    #[arg(long)]
    fold_recursive_prefix: bool,

    /// If a process produces jitdump or marker files, unlink them after
    /// opening. This ensures that the files will not be left in /tmp,
    /// but it will also be impossible to look at JIT disassembly, and line
    /// numbers will be missing for JIT frames.
    #[arg(long)]
    unlink_aux_files: bool,

    /// Create a separate thread for each CPU. Not supported on macOS
    #[arg(long)]
    per_cpu_threads: bool,

    /// Only include processes with these names
    #[arg(long)]
    process_names: Option<Vec<String>>,

    /// Only include processes with these PIDs
    #[arg(long)]
    pids: Option<Vec<u32>>,
}

fn main() {
    let opt = Opt::parse();
    match opt.action {
        Action::Load(load_args) => {
            let profile_filename = &load_args.file;
            let input_file = match File::open(profile_filename) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("Could not open file {:?}: {}", load_args.file, err);
                    std::process::exit(1)
                }
            };

            let libinfo_map =
                match parse_libinfo_map_from_profile_file(input_file, profile_filename) {
                    Ok(libinfo_map) => libinfo_map,
                    Err(err) => {
                        eprintln!("Could not parse the input file as JSON: {}", err);
                        eprintln!(
                            "If this is a perf.data file, please use `samply import` instead."
                        );
                        std::process::exit(1)
                    }
                };
            start_server_main(profile_filename, load_args.server_props(), libinfo_map);
        }

        Action::Import(import_args) => {
            let input_file = match File::open(&import_args.file) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("Could not open file {:?}: {}", import_args.file, err);
                    std::process::exit(1)
                }
            };
            let profile_creation_props = import_args.profile_creation_props();
            convert_file_to_profile(
                &import_args.file,
                &input_file,
                &import_args.output,
                profile_creation_props,
            );
            if let Some(server_props) = import_args.server_props() {
                let profile_filename = &import_args.output;
                let libinfo_map = profile_json_preparse::parse_libinfo_map_from_profile_file(
                    File::open(profile_filename).expect("Couldn't open file we just wrote"),
                    profile_filename,
                )
                .expect("Couldn't parse libinfo map from profile file");
                start_server_main(profile_filename, server_props, libinfo_map);
            }
        }

        #[cfg(any(
            target_os = "android",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))]
        Action::Record(record_args) => {
            let process_launch_props = record_args.process_launch_props();
            let recording_props = record_args.recording_props();
            let profile_creation_props = record_args.profile_creation_props();
            let server_props = record_args.server_props();

            if let Some(pid) = record_args.pid {
                profiler::start_profiling_pid(
                    pid,
                    recording_props,
                    profile_creation_props,
                    server_props,
                );
            } else {
                let exit_status = match profiler::start_recording(
                    process_launch_props,
                    recording_props,
                    profile_creation_props,
                    server_props,
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

impl LoadArgs {
    fn server_props(&self) -> ServerProps {
        self.server_args.server_props()
    }
}

impl ImportArgs {
    fn server_props(&self) -> Option<ServerProps> {
        if self.save_only {
            None
        } else {
            Some(self.server_args.server_props())
        }
    }

    fn profile_creation_props(&self) -> ProfileCreationProps {
        let profile_name = if let Some(profile_name) = &self.profile_creation_args.profile_name {
            profile_name.clone()
        } else {
            "Imported perf profile".to_string()
        };
        ProfileCreationProps {
            profile_name,
            reuse_threads: self.profile_creation_args.reuse_threads,
            fold_recursive_prefix: self.profile_creation_args.fold_recursive_prefix,
            unlink_aux_files: self.profile_creation_args.unlink_aux_files,
            create_per_cpu_threads: self.profile_creation_args.per_cpu_threads,
            include_process_names: None,
            include_process_ids: None,
        }
    }
}

impl RecordArgs {
    #[allow(unused)]
    fn server_props(&self) -> Option<ServerProps> {
        if self.save_only {
            None
        } else {
            Some(self.server_args.server_props())
        }
    }

    #[allow(unused)]
    pub fn recording_props(&self) -> RecordingProps {
        let time_limit = self.duration.map(Duration::from_secs_f64);
        if self.rate <= 0.0 {
            eprintln!(
                "Error: sampling rate must be greater than zero, got {}",
                self.rate
            );
            std::process::exit(1);
        }
        let interval = Duration::from_secs_f64(1.0 / self.rate);

        RecordingProps {
            output_file: self.output.clone(),
            time_limit,
            interval,
            main_thread_only: self.main_thread_only,
        }
    }

    pub fn process_launch_props(&self) -> ProcessLaunchProps {
        let command = &self.command;
        let iteration_count = self.iteration_count;
        assert!(
            !command.is_empty(),
            "CLI parsing should have ensured that we have at least one command name"
        );

        let mut env_vars = Vec::new();
        let mut i = 0;
        while let Some((var_name, var_val)) = command.get(i).and_then(|s| split_at_first_equals(s))
        {
            env_vars.push((var_name.to_owned(), var_val.to_owned()));
            i += 1;
        }
        if i == command.len() {
            eprintln!("Error: No command name found. Every item looks like an environment variable (contains '='): {command:?}");
            std::process::exit(1);
        }
        let command_name = command[i].clone();
        let args = command[(i + 1)..].to_owned();
        ProcessLaunchProps {
            env_vars,
            command_name,
            args,
            iteration_count,
        }
    }

    #[allow(unused)]
    pub fn profile_creation_props(&self) -> ProfileCreationProps {
        let profile_name = match (self.profile_creation_args.profile_name.clone(), self.pid) {
            (Some(profile_name), _) => profile_name,
            (None, Some(pid)) => format!("PID {pid}"),
            _ => self
                .process_launch_props()
                .command_name
                .to_string_lossy()
                .to_string(),
        };
        ProfileCreationProps {
            profile_name,
            reuse_threads: self.profile_creation_args.reuse_threads,
            fold_recursive_prefix: self.profile_creation_args.fold_recursive_prefix,
            unlink_aux_files: self.profile_creation_args.unlink_aux_files,
            create_per_cpu_threads: self.profile_creation_args.per_cpu_threads,
            include_process_names: self.profile_creation_args.process_names.clone(),
            include_process_ids: self.profile_creation_args.pids.clone(),
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

fn split_at_first_equals(s: &OsStr) -> Option<(&OsStr, &OsStr)> {
    let bytes = s.as_encoded_bytes();
    let pos = bytes.iter().position(|b| *b == b'=')?;
    let name = &bytes[..pos];
    let val = &bytes[(pos + 1)..];
    // SAFETY:
    // - `name` and `val` only contain content that originated from `OsStr::as_encoded_bytes`
    // - Only split with ASCII '=' which is a non-empty UTF-8 substring
    let (name, val) = unsafe {
        (
            OsStr::from_encoded_bytes_unchecked(name),
            OsStr::from_encoded_bytes_unchecked(val),
        )
    };
    Some((name, val))
}

fn convert_file_to_profile(
    filename: &Path,
    input_file: &File,
    output_filename: &Path,
    profile_creation_props: ProfileCreationProps,
) {
    if filename.extension() == Some(OsStr::new("etl")) {
        convert_etl_file_to_profile(
            filename,
            input_file,
            output_filename,
            profile_creation_props,
        );
        return;
    }

    convert_perf_data_file_to_profile(
        filename,
        input_file,
        output_filename,
        profile_creation_props,
    );
}

#[cfg(target_os = "windows")]
fn convert_etl_file_to_profile(
    filename: &Path,
    _input_file: &File,
    output_filename: &Path,
    profile_creation_props: ProfileCreationProps,
) {
    windows::import::convert_etl_file_to_profile(filename, output_filename, profile_creation_props);
}

#[cfg(not(target_os = "windows"))]
fn convert_etl_file_to_profile(
    filename: &Path,
    _input_file: &File,
    _output_filename: &Path,
    _profile_creation_props: ProfileCreationProps,
) {
    eprintln!(
        "Error: Could not import ETW trace from file {}",
        filename.to_string_lossy()
    );
    eprintln!("Importing ETW traces is only supported on Windows.");
    std::process::exit(1);
}

fn convert_perf_data_file_to_profile(
    filename: &Path,
    input_file: &File,
    output_filename: &Path,
    profile_creation_props: ProfileCreationProps,
) {
    let path = Path::new(filename)
        .canonicalize()
        .expect("Couldn't form absolute path");
    let file_meta = input_file.metadata().ok();
    let file_mod_time = file_meta.and_then(|metadata| metadata.modified().ok());
    let reader = BufReader::new(input_file);
    let profile =
        match import::perf::convert(reader, file_mod_time, path.parent(), profile_creation_props) {
            Ok(profile) => profile,
            Err(error) => {
                eprintln!("Error importing perf.data file: {:?}", error);
                std::process::exit(1);
            }
        };
    let output_file = match File::create(output_filename) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Couldn't create output file {:?}: {}", output_filename, err);
            std::process::exit(1);
        }
    };
    let writer = BufWriter::new(output_file);
    serde_json::to_writer(writer, &profile).expect("Couldn't write converted profile JSON");
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Opt::command().debug_assert();
    }

    #[cfg(any(target_os = "android", target_os = "macos", target_os = "linux"))]
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
