use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use super::cli_utils::{parse_time_range, split_at_first_equals};
use super::server::{PortSelection, ServerProps};
use super::shared::included_processes::IncludedProcesses;
use super::shared::prop_types::{
    CoreClrProfileProps, ImportProps, ProcessLaunchProps, ProfileCreationProps, RecordingMode,
    RecordingProps, SymbolProps,
};

#[derive(Debug, Parser)]
#[command(
    name = "samply",
    version,
    about = r#"
samply is a sampling CPU profiler for Windows, macOS, and Linux.
See "samply record --help" for additional information about the "samply record" command.

EXAMPLES:
    # Profile a freshly launched process:
    samply record ./yourcommand yourargs

    # Profile an existing process by pid:
    samply record -p 12345

    # Alternative usage: Save profile to file for later viewing, and then load it.
    samply record --save-only -o prof.json -- ./yourcommand yourargs
    samply load prof.json # Opens in the browser and supplies symbols

    # Import perf.data files from Linux perf or Android simpleperf:
    samply import perf.data
"#
)]
pub struct Opt {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Debug, Subcommand)]
pub enum Action {
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

    #[cfg(target_os = "windows")]
    #[clap(hide = true)]
    /// Used in the elevated helper process.
    RunElevatedHelper(RunElevatedHelperArgs),

    /// Codesign the samply binary on macOS to allow attaching to processes.
    #[cfg(target_os = "macos")]
    Setup(SetupArgs),
}

#[derive(Debug, Args)]
pub struct LoadArgs {
    /// Path to the file that should be loaded.
    pub file: PathBuf,

    #[command(flatten)]
    pub server_args: ServerArgs,

    #[command(flatten)]
    pub symbol_args: SymbolArgs,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Path to the profile file that should be imported.
    pub file: PathBuf,

    /// Optional extra paths to ETL files for user sessions.
    pub user_etl: Vec<PathBuf>,

    #[command(flatten)]
    pub profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    pub save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json.gz")]
    pub output: PathBuf,

    #[command(flatten)]
    pub server_args: ServerArgs,

    #[command(flatten)]
    pub symbol_args: SymbolArgs,

    /// Additional directories to use for looking up jitdump and marker files.
    #[arg(long)]
    pub aux_file_dir: Vec<PathBuf>,

    /// Only include processes with this name substring (can be specified multiple times).
    #[arg(long)]
    pub name: Option<Vec<String>>,

    /// Only include process with this PID (can be specified multiple times).
    #[arg(long)]
    pub pid: Option<Vec<u32>>,

    /// Explicitly specify architecture of profile to import.
    #[arg(long)]
    pub override_arch: Option<String>,

    /// Time range of recording to include in profile. Format is "start-stop" or "start+duration" with each part optional, e.g. "5s", "5s-", "-10s", "1s-10s" or "1s+9s".
    #[arg(long, value_parser=parse_time_range)]
    pub time_range: Option<(std::time::Duration, std::time::Duration)>,
}

#[allow(unused)]
#[derive(Debug, Args)]
pub struct RecordArgs {
    /// Sampling rate, in Hz
    #[arg(short, long, default_value = "1000")]
    pub rate: f64,

    /// Limit the recorded time to the specified number of seconds
    #[arg(short, long)]
    pub duration: Option<f64>,

    /// How many times to run the profiled command.
    #[arg(long, default_value = "1")]
    pub iteration_count: u32,

    /// Ignore exit code and continue running when iteration_count > 0
    #[arg(short, long)]
    pub ignore_exit_code: bool,

    #[command(flatten)]
    pub profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    pub save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json.gz")]
    pub output: PathBuf,

    #[command(flatten)]
    pub server_args: ServerArgs,

    #[command(flatten)]
    pub symbol_args: SymbolArgs,

    /// Profile the execution of this command.
    #[arg(
        required_unless_present_any = ["pid", "all"],
        conflicts_with_all = ["pid", "all"],
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    pub command: Vec<std::ffi::OsString>,

    /// Process ID of existing process to attach to.
    #[arg(short, long, conflicts_with = "all")]
    pub pid: Option<u32>,

    /// Profile entire system (all processes). Not supported on macOS.
    #[arg(short, long, conflicts_with = "pid")]
    pub all: bool,

    /// VM hack for arm64 Windows VMs to not try to record PROFILE events (Windows only).
    #[cfg(target_os = "windows")]
    #[arg(long)]
    pub vm_hack: bool,

    /// Enable Graphics-related event capture.
    #[arg(long)]
    pub gfx: bool,

    /// Enable browser-related event capture (JavaScript stacks and trace events)
    #[arg(long)]
    pub browsers: bool,

    /// Keep the ETL file after recording (Windows only).
    #[cfg(target_os = "windows")]
    #[arg(long)]
    pub keep_etl: bool,
}

#[derive(ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoreClrArgs {
    Enabled,
    #[cfg(target_os = "windows")]
    GcMarkers,
    #[cfg(target_os = "windows")]
    GcSuspendedThreads,
    #[cfg(target_os = "windows")]
    GcDetailedAllocs,
    #[cfg(target_os = "windows")]
    EventStacks,
}

impl std::fmt::Display for CoreClrArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_possible_value()
            .expect("no values are skipped")
            .get_name()
            .fmt(f)
    }
}

#[derive(Debug, Args)]
pub struct ServerArgs {
    /// Do not open the profiler UI.
    #[arg(short, long)]
    pub no_open: bool,

    /// The address to use for the local web server
    #[arg(long, default_value = "127.0.0.1")]
    pub address: String,

    /// The port to use for the local web server
    #[arg(short = 'P', long, default_value = "3000+")]
    pub port: String,

    /// Print debugging output.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Arguments describing where to obtain symbol files.
#[derive(Debug, Args)]
pub struct SymbolArgs {
    /// Extra directories containing symbol files
    #[arg(long)]
    pub symbol_dir: Vec<PathBuf>,

    /// Additional URLs of symbol servers serving PDB / DLL / EXE files
    #[arg(long)]
    pub windows_symbol_server: Vec<String>,

    /// Overrides the default cache directory for Windows symbol files which were downloaded from a symbol server
    #[arg(long)]
    pub windows_symbol_cache: Option<PathBuf>,

    /// Additional URLs of symbol servers serving Breakpad .sym files
    #[arg(long)]
    pub breakpad_symbol_server: Vec<String>,

    /// Additional local directories containing Breakpad .sym files
    #[arg(long)]
    pub breakpad_symbol_dir: Vec<String>,

    /// Overrides the default cache directory for Breakpad symbol files
    #[arg(long)]
    pub breakpad_symbol_cache: Option<PathBuf>,

    /// Extra directory containing symbol files, with the directory structure used by simpleperf's scripts
    #[arg(long)]
    pub simpleperf_binary_cache: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
pub struct SetupArgs {
    /// Don't wait for confirmation to codesign.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[derive(Debug, Args, Clone)]
pub struct ProfileCreationArgs {
    /// Set a custom name for the recorded profile.
    /// By default it is either the command that was run or the process pid.
    #[arg(long)]
    pub profile_name: Option<String>,

    /// Only include the main thread of each process in order to reduce profile size,
    /// only respected on Windows and macOS
    #[arg(long)]
    pub main_thread_only: bool,

    /// Merge non-overlapping threads of the same name.
    #[arg(long)]
    pub reuse_threads: bool,

    /// Fold repeated frames at the base of the stack.
    #[arg(long)]
    pub fold_recursive_prefix: bool,

    /// If a process produces jitdump or marker files, unlink them after
    /// opening. This ensures that the files will not be left in /tmp,
    /// but it will also be impossible to look at JIT disassembly, and line
    /// numbers will be missing for JIT frames.
    #[arg(long)]
    pub unlink_aux_files: bool,

    /// Create a separate thread for each CPU. Not supported on macOS
    #[arg(long)]
    pub per_cpu_threads: bool,

    /// Emit a JitFunctionAdd markers when a JIT function is added.
    #[arg(long)]
    pub jit_markers: bool,

    /// Emit context switch markers.
    #[arg(long)]
    pub cswitch_markers: bool,

    /// Include up to <INCLUDE_ARGS> command line arguments in the process name.
    /// This can help differentiate processes if the same executable is used
    /// for different types of programs. And in --reuse-threads mode it
    /// allows more control over which processes are matched up.
    #[arg(long, default_value = "0", num_args=0..=1, require_equals = true, default_missing_value = "100")]
    pub include_args: usize,

    /// Emit .syms.json sidecar file containing gathered symbol info for all frames referenced by
    /// this profile. With this file along with the profile, samply can load the profile
    /// and provide symbols to the front end without needing debug files to be
    /// available. (Unstable: will probably change to include the full information
    /// in the profile.json, instead of a sidecar file.)
    #[arg(long)]
    pub unstable_presymbolicate: bool,

    /// Emit markers for any unknown ETW events that are encountered.
    #[cfg(target_os = "windows")]
    #[arg(long)]
    pub unknown_event_markers: bool,

    /// Enable CoreCLR event conversion.
    #[clap(long, require_equals = true, value_name = "FLAG", value_enum, value_delimiter = ',', num_args = 0.., default_values_t = vec![CoreClrArgs::Enabled])]
    pub coreclr: Vec<CoreClrArgs>,
}

#[derive(Debug, Args)]
pub struct RunElevatedHelperArgs {
    #[arg(long)]
    pub ipc_directory: PathBuf,

    #[arg(long)]
    pub output_path: PathBuf,
}

impl LoadArgs {
    pub fn server_props(&self) -> ServerProps {
        self.server_args.server_props()
    }

    pub fn symbol_props(&self) -> SymbolProps {
        self.symbol_args.symbol_props()
    }
}

impl ImportArgs {
    pub fn server_props(&self) -> Option<ServerProps> {
        if self.save_only {
            None
        } else {
            Some(self.server_args.server_props())
        }
    }

    pub fn symbol_props(&self) -> SymbolProps {
        self.symbol_args.symbol_props()
    }

    pub fn profile_creation_props(&self) -> ProfileCreationProps {
        let filename = self.file.file_name().unwrap_or(self.file.as_os_str());
        let fallback_profile_name = filename.to_string_lossy().into();
        self.profile_creation_args
            .profile_creation_props_with_fallback_name(fallback_profile_name)
    }

    // TODO: Use for perf.data import
    #[allow(unused)]
    pub fn included_processes(&self) -> Option<IncludedProcesses> {
        match (&self.name, &self.pid) {
            (None, None) => None, // No filtering, include all processes
            (names, pids) => Some(IncludedProcesses {
                name_substrings: names.clone().unwrap_or_default(),
                pids: pids.clone().unwrap_or_default(),
            }),
        }
    }

    pub fn import_props(&self) -> ImportProps {
        ImportProps {
            profile_creation_props: self.profile_creation_props(),
            symbol_props: self.symbol_props(),
            included_processes: self.included_processes(),
            user_etl: self.user_etl.clone(),
            aux_file_dir: self.aux_file_dir.clone(),
            time_range: self.time_range,
        }
    }
}

impl RecordArgs {
    #[allow(unused)]
    pub fn server_props(&self) -> Option<ServerProps> {
        if self.save_only {
            None
        } else {
            Some(self.server_args.server_props())
        }
    }

    pub fn symbol_props(&self) -> SymbolProps {
        self.symbol_args.symbol_props()
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
            gfx: self.gfx,
            browsers: self.browsers,
            #[cfg(target_os = "windows")]
            vm_hack: self.vm_hack,
            #[cfg(not(target_os = "windows"))]
            vm_hack: false,
            #[cfg(target_os = "windows")]
            keep_etl: self.keep_etl,
            #[cfg(not(target_os = "windows"))]
            keep_etl: false,
        }
    }

    pub fn recording_mode(&self) -> RecordingMode {
        let (command, iteration_count) = match (self.all, &self.pid) {
            (true, _) => return RecordingMode::All,
            (false, Some(pid)) => return RecordingMode::Pid(*pid),
            (false, None) => (&self.command, self.iteration_count),
        };

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
        let launch_props = ProcessLaunchProps {
            env_vars,
            command_name,
            args,
            iteration_count,
            ignore_exit_code: self.ignore_exit_code,
        };

        RecordingMode::Launch(launch_props)
    }

    pub fn profile_creation_props(&self) -> ProfileCreationProps {
        let fallback_profile_name = match self.recording_mode() {
            RecordingMode::All => "All processes".to_string(),
            RecordingMode::Pid(pid) => format!("PID {pid}"),
            RecordingMode::Launch(launch_props) => {
                let filename = Path::new(&launch_props.command_name)
                    .file_name()
                    .unwrap_or(launch_props.command_name.as_os_str());
                filename.to_string_lossy().into()
            }
        };
        self.profile_creation_args
            .profile_creation_props_with_fallback_name(fallback_profile_name)
    }
}

impl ProfileCreationArgs {
    pub fn coreclr_profile_props(&self) -> CoreClrProfileProps {
        // on Windows, the ..Default::default() has no effect, and clippy doesn't like it
        #[allow(clippy::needless_update)]
        CoreClrProfileProps {
            enabled: self.coreclr.contains(&CoreClrArgs::Enabled),
            #[cfg(target_os = "windows")]
            gc_markers: self.coreclr.contains(&CoreClrArgs::GcMarkers),
            #[cfg(target_os = "windows")]
            gc_suspensions: self.coreclr.contains(&CoreClrArgs::GcSuspendedThreads),
            #[cfg(target_os = "windows")]
            gc_detailed_allocs: self.coreclr.contains(&CoreClrArgs::GcDetailedAllocs),
            #[cfg(target_os = "windows")]
            event_stacks: self.coreclr.contains(&CoreClrArgs::EventStacks),
            ..Default::default()
        }
    }

    pub fn profile_creation_props_with_fallback_name(
        &self,
        fallback_profile_name: String,
    ) -> ProfileCreationProps {
        ProfileCreationProps {
            profile_name: self.profile_name.clone(),
            fallback_profile_name,
            main_thread_only: self.main_thread_only,
            reuse_threads: self.reuse_threads,
            fold_recursive_prefix: self.fold_recursive_prefix,
            unlink_aux_files: self.unlink_aux_files,
            create_per_cpu_threads: self.per_cpu_threads,
            arg_count_to_include_in_process_name: self.include_args,
            override_arch: None,
            unstable_presymbolicate: self.unstable_presymbolicate,
            should_emit_jit_markers: self.jit_markers,
            should_emit_cswitch_markers: self.cswitch_markers,
            coreclr: self.coreclr_profile_props(),
            #[cfg(target_os = "windows")]
            unknown_event_markers: self.unknown_event_markers,
            #[cfg(not(target_os = "windows"))]
            unknown_event_markers: false,
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

        // parse address from string
        let address = match IpAddr::from_str(&self.address) {
            Ok(addr) => addr,
            Err(e) => {
                eprintln!(
                    "Could not parse address as IpAddr, got address {:?}, error: {}",
                    self.address, e
                );
                std::process::exit(1)
            }
        };

        ServerProps {
            address,
            port_selection,
            verbose: self.verbose,
            open_in_browser,
        }
    }
}

impl SymbolArgs {
    pub fn symbol_props(&self) -> SymbolProps {
        SymbolProps {
            symbol_dir: self.symbol_dir.clone(),
            windows_symbol_server: self.windows_symbol_server.clone(),
            windows_symbol_cache: self.windows_symbol_cache.clone(),
            breakpad_symbol_server: self.breakpad_symbol_server.clone(),
            breakpad_symbol_dir: self.breakpad_symbol_dir.clone(),
            breakpad_symbol_cache: self.breakpad_symbol_cache.clone(),
            simpleperf_binary_cache: self.simpleperf_binary_cache.clone(),
        }
    }
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
