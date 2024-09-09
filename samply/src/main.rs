#[cfg(target_os = "macos")]
mod mac;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

mod import;
mod linux_shared;
mod name;
mod profile_json_preparse;
mod server;
mod shared;

use std::ffi::OsStr;
use std::fs::File;
use std::io::BufReader;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
#[cfg(any(target_os = "android", target_os = "linux"))]
use linux::profiler;
#[cfg(target_os = "macos")]
use mac::profiler;
// To avoid warnings about unused declarations
#[cfg(target_os = "macos")]
pub use mac::{kernel_error, thread_act, thread_info};
use profile_json_preparse::parse_libinfo_map_from_profile_file;
use server::{start_server_main, PortSelection, ServerProps};
use shared::included_processes::IncludedProcesses;
use shared::recording_props::{
    CoreClrProfileProps, ProcessLaunchProps, ProfileCreationProps, RecordingMode, RecordingProps,
};
use shared::save_profile::save_profile_to_file;
use shared::symbol_props::SymbolProps;
#[cfg(target_os = "windows")]
use windows::profiler;

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

    #[cfg(target_os = "windows")]
    #[clap(hide = true)]
    /// Used in the elevated helper process.
    RunElevatedHelper(RunElevatedHelperArgs),

    /// Codesign the samply binary on macOS to allow attaching to processes.
    #[cfg(target_os = "macos")]
    Setup(SetupArgs),
}

#[derive(Debug, Args)]
struct LoadArgs {
    /// Path to the file that should be loaded.
    file: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,

    #[command(flatten)]
    symbol_args: SymbolArgs,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// Path to the profile file that should be imported.
    file: PathBuf,

    /// Optional extra paths to ETL files for user sessions.
    user_etl: Vec<PathBuf>,

    #[command(flatten)]
    profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json.gz")]
    output: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,

    #[command(flatten)]
    symbol_args: SymbolArgs,

    /// Additional directories to use for looking up jitdump and marker files.
    #[arg(long)]
    aux_file_dir: Vec<PathBuf>,

    /// Only include processes with this name substring (can be specified multiple times).
    #[arg(long)]
    name: Option<Vec<String>>,

    /// Only include process with this PID (can be specified multiple times).
    #[arg(long)]
    pid: Option<Vec<u32>>,

    /// Explicitly specify architecture of profile to import.
    #[arg(long)]
    override_arch: Option<String>,

    /// Enable CoreCLR event conversion.
    #[clap(long, require_equals = true, value_name = "FLAG", value_enum, value_delimiter = ',', num_args = 0.., default_values_t = vec![CoreClrArgs::Enabled])]
    coreclr: Vec<CoreClrArgs>,

    /// Time range of recording to include in profile. Format is "start-stop" or "start+duration" with each part optional, e.g. "5s", "5s-", "-10s", "1s-10s" or "1s+9s".
    #[cfg(target_os = "windows")]
    #[arg(long, value_parser=parse_time_range)]
    time_range: Option<(std::time::Duration, std::time::Duration)>,
}

#[allow(unused)]
fn parse_time_range(
    arg: &str,
) -> Result<(std::time::Duration, std::time::Duration), humantime::DurationError> {
    let (is_duration, splitchar) = if arg.contains('+') {
        (true, '+')
    } else {
        (false, '-')
    };

    let parts: Vec<&str> = arg.splitn(2, splitchar).collect();

    let start = if parts[0].is_empty() {
        std::time::Duration::ZERO
    } else {
        humantime::parse_duration(parts[0])?
    };

    let end = if parts.len() == 1 || parts[1].is_empty() {
        std::time::Duration::MAX
    } else {
        humantime::parse_duration(parts[1])?
    };

    Ok((start, if is_duration { start + end } else { end }))
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

    #[command(flatten)]
    profile_creation_args: ProfileCreationArgs,

    /// Do not run a local server after recording.
    #[arg(short, long)]
    save_only: bool,

    /// Output filename.
    #[arg(short, long, default_value = "profile.json.gz")]
    output: PathBuf,

    #[command(flatten)]
    server_args: ServerArgs,

    #[command(flatten)]
    symbol_args: SymbolArgs,

    /// Profile the execution of this command.
    #[arg(
        required_unless_present_any = ["pid", "all"],
        conflicts_with_all = ["pid", "all"],
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    command: Vec<std::ffi::OsString>,

    /// Process ID of existing process to attach to.
    #[arg(short, long, conflicts_with = "all")]
    pid: Option<u32>,

    /// Profile entire system (all processes). Not supported on macOS.
    #[arg(short, long, conflicts_with = "pid")]
    all: bool,

    /// Enable CoreCLR event capture.
    #[clap(long, require_equals = true, value_name = "FLAG", value_enum, value_delimiter = ',', num_args = 0.., default_missing_value = "enabled")]
    coreclr: Vec<CoreClrArgs>,

    /// VM hack for arm64 Windows VMs to not try to record PROFILE events (Windows only).
    #[cfg(target_os = "windows")]
    #[arg(long)]
    vm_hack: bool,

    /// Enable Graphics-related event capture.
    #[arg(long)]
    gfx: bool,

    /// Enable browser-related event capture (JavaScript stacks and trace events)
    #[arg(long)]
    browsers: bool,

    /// Keep the ETL file after recording (Windows only).
    #[cfg(target_os = "windows")]
    #[arg(long)]
    keep_etl: bool,
}

#[derive(ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
enum CoreClrArgs {
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
struct ServerArgs {
    /// Do not open the profiler UI.
    #[arg(short, long)]
    no_open: bool,

    /// The address to use for the local web server
    #[arg(long, default_value = "127.0.0.1")]
    address: String,

    /// The port to use for the local web server
    #[arg(short = 'P', long, default_value = "3000+")]
    port: String,

    /// Print debugging output.
    #[arg(short, long)]
    verbose: bool,
}

/// Arguments describing where to obtain symbol files.
#[derive(Debug, Args)]
struct SymbolArgs {
    /// Extra directories containing symbol files
    #[arg(long)]
    symbol_dir: Vec<PathBuf>,

    /// Additional URLs of symbol servers serving PDB / DLL / EXE files
    #[arg(long)]
    windows_symbol_server: Vec<String>,

    /// Overrides the default cache directory for Windows symbol files which were downloaded from a symbol server
    #[arg(long)]
    windows_symbol_cache: Option<PathBuf>,

    /// Additional URLs of symbol servers serving Breakpad .sym files
    #[arg(long)]
    breakpad_symbol_server: Vec<String>,

    /// Additional local directories containing Breakpad .sym files
    #[arg(long)]
    breakpad_symbol_dir: Vec<String>,

    /// Overrides the default cache directory for Breakpad symbol files
    #[arg(long)]
    breakpad_symbol_cache: Option<PathBuf>,

    /// Extra directory containing symbol files, with the directory structure used by simpleperf's scripts
    #[arg(long)]
    simpleperf_binary_cache: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
pub struct SetupArgs {
    /// Don't wait for confirmation to codesign.
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Debug, Args, Clone)]
pub struct ProfileCreationArgs {
    /// Set a custom name for the recorded profile.
    /// By default it is either the command that was run or the process pid.
    #[arg(long)]
    profile_name: Option<String>,

    /// Only include the main thread of each process in order to reduce profile size,
    /// only respected on Windows and macOS
    #[arg(long)]
    main_thread_only: bool,

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

    /// Include up to <INCLUDE_ARGS> command line arguments in the process name.
    /// This can help differentiate processes if the same executable is used
    /// for different types of programs. And in --reuse-threads mode it
    /// allows more control over which processes are matched up.
    #[arg(long, default_value = "0", num_args=0..=1, require_equals = true, default_missing_value = "100")]
    include_args: usize,

    /// Emit .syms.json sidecar file containing gathered symbol info for all frames referenced by
    /// this profile. With this file along with the profile, samply can load the profile
    /// and provide symbols to the front end without needing debug files to be
    /// available. (Unstable: will probably change to include the full information
    /// in the profile.json, instead of a sidecar file.)
    #[arg(long)]
    unstable_presymbolicate: bool,

    /// Emit markers for any unknown ETW events that are encountered.
    #[cfg(target_os = "windows")]
    #[arg(long)]
    unknown_event_markers: bool,
}

#[derive(Debug, Args)]
struct RunElevatedHelperArgs {
    #[arg(long)]
    ipc_directory: PathBuf,

    #[arg(long)]
    output_path: PathBuf,
}

fn main() {
    env_logger::init();

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
            start_server_main(
                profile_filename,
                load_args.server_props(),
                load_args.symbol_props(),
                libinfo_map,
            );
        }

        Action::Import(import_args) => {
            let input_file = match File::open(&import_args.file) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("Could not open file {:?}: {}", import_args.file, err);
                    std::process::exit(1)
                }
            };
            convert_file_to_profile(&input_file, &import_args);
            if let Some(server_props) = import_args.server_props() {
                let profile_filename = &import_args.output;
                let libinfo_map = profile_json_preparse::parse_libinfo_map_from_profile_file(
                    File::open(profile_filename).expect("Couldn't open file we just wrote"),
                    profile_filename,
                )
                .expect("Couldn't parse libinfo map from profile file");
                start_server_main(
                    profile_filename,
                    server_props,
                    import_args.symbol_props(),
                    libinfo_map,
                );
            }
        }

        #[cfg(any(
            target_os = "android",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))]
        Action::Record(record_args) => {
            let recording_props = record_args.recording_props();
            let recording_mode = record_args.recording_mode();
            let profile_creation_props = record_args.profile_creation_props();
            let symbol_props = record_args.symbol_props();
            let server_props = record_args.server_props();

            let exit_status = match profiler::start_recording(
                recording_mode,
                recording_props,
                profile_creation_props,
                symbol_props,
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

        #[cfg(target_os = "windows")]
        Action::RunElevatedHelper(RunElevatedHelperArgs {
            ipc_directory,
            output_path,
        }) => {
            windows::run_elevated_helper(&ipc_directory, output_path);
        }

        #[cfg(target_os = "macos")]
        Action::Setup(SetupArgs { yes }) => {
            mac::codesign_setup::codesign_setup(yes);
        }
    }
}

impl LoadArgs {
    fn server_props(&self) -> ServerProps {
        self.server_args.server_props()
    }

    fn symbol_props(&self) -> SymbolProps {
        self.symbol_args.symbol_props()
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

    fn symbol_props(&self) -> SymbolProps {
        self.symbol_args.symbol_props()
    }

    fn profile_creation_props(&self) -> ProfileCreationProps {
        let filename = self.file.file_name().unwrap_or(self.file.as_os_str());
        let fallback_profile_name = filename.to_string_lossy().into();
        ProfileCreationProps {
            profile_name: self.profile_creation_args.profile_name.clone(),
            fallback_profile_name,
            main_thread_only: self.profile_creation_args.main_thread_only,
            reuse_threads: self.profile_creation_args.reuse_threads,
            fold_recursive_prefix: self.profile_creation_args.fold_recursive_prefix,
            unlink_aux_files: self.profile_creation_args.unlink_aux_files,
            create_per_cpu_threads: self.profile_creation_args.per_cpu_threads,
            arg_count_to_include_in_process_name: self.profile_creation_args.include_args,
            override_arch: self.override_arch.clone(),
            unstable_presymbolicate: self.profile_creation_args.unstable_presymbolicate,
            coreclr: to_coreclr_profile_props(&self.coreclr),
            #[cfg(target_os = "windows")]
            unknown_event_markers: self.profile_creation_args.unknown_event_markers,
            #[cfg(not(target_os = "windows"))]
            unknown_event_markers: false,
            #[cfg(target_os = "windows")]
            time_range: self.time_range,
            #[cfg(not(target_os = "windows"))]
            time_range: None,
        }
    }

    // TODO: Use for perf.data import
    #[allow(unused)]
    fn included_processes(&self) -> Option<IncludedProcesses> {
        match (&self.name, &self.pid) {
            (None, None) => None, // No filtering, include all processes
            (names, pids) => Some(IncludedProcesses {
                name_substrings: names.clone().unwrap_or_default(),
                pids: pids.clone().unwrap_or_default(),
            }),
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

    fn symbol_props(&self) -> SymbolProps {
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
        ProfileCreationProps {
            profile_name: self.profile_creation_args.profile_name.clone(),
            fallback_profile_name,
            main_thread_only: self.profile_creation_args.main_thread_only,
            reuse_threads: self.profile_creation_args.reuse_threads,
            fold_recursive_prefix: self.profile_creation_args.fold_recursive_prefix,
            unlink_aux_files: self.profile_creation_args.unlink_aux_files,
            create_per_cpu_threads: self.profile_creation_args.per_cpu_threads,
            arg_count_to_include_in_process_name: self.profile_creation_args.include_args,
            override_arch: None,
            unstable_presymbolicate: self.profile_creation_args.unstable_presymbolicate,
            coreclr: to_coreclr_profile_props(&self.coreclr),
            #[cfg(target_os = "windows")]
            unknown_event_markers: self.profile_creation_args.unknown_event_markers,
            #[cfg(not(target_os = "windows"))]
            unknown_event_markers: false,
            time_range: None,
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

fn to_coreclr_profile_props(coreclr_args: &[CoreClrArgs]) -> CoreClrProfileProps {
    // on Windows, the ..Default::default() has no effect, and clippy doesn't like it
    #[allow(clippy::needless_update)]
    CoreClrProfileProps {
        enabled: coreclr_args.contains(&CoreClrArgs::Enabled),
        #[cfg(target_os = "windows")]
        gc_markers: coreclr_args.contains(&CoreClrArgs::GcMarkers),
        #[cfg(target_os = "windows")]
        gc_suspensions: coreclr_args.contains(&CoreClrArgs::GcSuspendedThreads),
        #[cfg(target_os = "windows")]
        gc_detailed_allocs: coreclr_args.contains(&CoreClrArgs::GcDetailedAllocs),
        #[cfg(target_os = "windows")]
        event_stacks: coreclr_args.contains(&CoreClrArgs::EventStacks),
        ..Default::default()
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

fn convert_file_to_profile(input_file: &File, import_args: &ImportArgs) {
    if import_args.file.extension() == Some(OsStr::new("etl")) {
        convert_etl_file_to_profile(input_file, import_args);
        return;
    }

    convert_perf_data_file_to_profile(input_file, import_args);
}

#[cfg(target_os = "windows")]
fn convert_etl_file_to_profile(_input_file: &File, import_args: &ImportArgs) {
    let profile_creation_props = import_args.profile_creation_props();
    let included_processes = import_args.included_processes();
    windows::import::convert_etl_file_to_profile(
        &import_args.file,
        &import_args.user_etl,
        &import_args.output,
        profile_creation_props,
        included_processes,
    );
}

#[cfg(not(target_os = "windows"))]
fn convert_etl_file_to_profile(_input_file: &File, import_args: &ImportArgs) {
    eprintln!(
        "Error: Could not import ETW trace from file {}",
        import_args.file.to_string_lossy()
    );
    eprintln!("Importing ETW traces is only supported on Windows.");
    std::process::exit(1);
}

fn convert_perf_data_file_to_profile(input_file: &File, import_args: &ImportArgs) {
    let path = import_args
        .file
        .canonicalize()
        .expect("Couldn't form absolute path");
    let file_meta = input_file.metadata().ok();
    let file_mod_time = file_meta.and_then(|metadata| metadata.modified().ok());
    let profile_creation_props = import_args.profile_creation_props();
    let mut binary_lookup_dirs = import_args.symbol_props().symbol_dir.clone();
    let mut aux_file_lookup_dirs = import_args.aux_file_dir.clone();
    if let Some(parent_dir) = path.parent() {
        binary_lookup_dirs.push(parent_dir.into());
        aux_file_lookup_dirs.push(parent_dir.into());
    }
    let reader = BufReader::new(input_file);
    let profile = match import::perf::convert(
        reader,
        file_mod_time,
        binary_lookup_dirs,
        aux_file_lookup_dirs,
        profile_creation_props,
    ) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("Error importing perf.data file: {:?}", error);
            std::process::exit(1);
        }
    };
    save_profile_to_file(&profile, &import_args.output).expect("Couldn't write JSON");
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
