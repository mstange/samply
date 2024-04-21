use std::{ffi::OsString, path::PathBuf, time::Duration};

/// Properties which are meaningful both for recording a fresh process
/// as well as for recording an existing process.
pub struct RecordingProps {
    pub output_file: PathBuf,
    pub time_limit: Option<Duration>,
    pub interval: Duration,
    pub main_thread_only: bool,
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
}

/// Properties which are meaningful for launching and recording a fresh process.
pub struct ProcessLaunchProps {
    pub env_vars: Vec<(OsString, OsString)>,
    pub command_name: OsString,
    pub args: Vec<OsString>,
    pub iteration_count: u32,
}
