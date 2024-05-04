use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// Properties which are meaningful both for recording a fresh process
/// as well as for recording an existing process.
#[derive(Debug, Clone)]
pub struct RecordingProps {
    pub output_file: PathBuf,
    pub time_limit: Option<Duration>,
    pub interval: Duration,
    pub main_thread_only: bool,
    pub coreclr: bool,
    pub coreclr_allocs: bool,
    pub vm_hack: bool,
}

/// Which process(es) to record.
pub enum RecordingMode {
    /// Record all processes, system-wide.
    All,
    /// Record just a single process (and its children).
    Pid(u32),
    /// Launch a process, and record just that process (and its children).
    Launch(ProcessLaunchProps),
}

/// Properties which are meaningful both for recording a profile and
/// for converting a perf.data file to a profile.
pub struct ProfileCreationProps {
    pub profile_name: String,
    /// Merge non-overlapping threads of the same name.
    pub reuse_threads: bool,
    /// Fold repeated frames at the base of the stack.
    pub fold_recursive_prefix: bool,
    /// Unlink jitdump/marker files
    pub unlink_aux_files: bool,
    /// Create a separate thread for each CPU.
    pub create_per_cpu_threads: bool,
    /// Override system architecture.
    #[allow(dead_code)]
    pub override_arch: Option<String>,
}

/// Properties which are meaningful for launching and recording a fresh process.
pub struct ProcessLaunchProps {
    pub env_vars: Vec<(OsString, OsString)>,
    pub command_name: OsString,
    pub args: Vec<OsString>,
    pub iteration_count: u32,
}
