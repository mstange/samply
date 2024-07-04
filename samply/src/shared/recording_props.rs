use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct CoreClrProfileProps {
    pub enabled: bool,
    pub gc_markers: bool,
    pub gc_suspensions: bool,
    pub gc_detailed_allocs: bool,
    pub event_stacks: bool,
}

impl CoreClrProfileProps {
    pub fn any_enabled(&self) -> bool {
        self.enabled
            || self.gc_markers
            || self.gc_suspensions
            || self.gc_detailed_allocs
            || self.event_stacks
    }
}

/// Properties which are meaningful both for recording a fresh process
/// as well as for recording an existing process.
#[derive(Debug, Clone)]
pub struct RecordingProps {
    pub output_file: PathBuf,
    pub time_limit: Option<Duration>,
    pub interval: Duration,
    #[allow(dead_code)]
    pub vm_hack: bool,
    #[allow(dead_code)]
    pub gfx: bool,
    #[allow(dead_code)]
    pub browsers: bool,
    #[allow(dead_code)]
    pub keep_etl: bool,
}

/// Which process(es) to record.
#[derive(Debug, Clone)]
pub enum RecordingMode {
    /// Record all processes, system-wide.
    All,
    /// Record just a single process (and its children).
    Pid(u32),
    /// Launch a process, and record just that process (and its children).
    Launch(ProcessLaunchProps),
}

impl RecordingMode {
    #[allow(dead_code)]
    pub fn is_attach_mode(&self) -> bool {
        match self {
            RecordingMode::All => true,
            RecordingMode::Pid(_) => true,
            RecordingMode::Launch(_) => false,
        }
    }
}

/// Properties which are meaningful both for recording a profile and
/// for converting a perf.data / ETL file to a profile.
#[derive(Debug, Clone)]
pub struct ProfileCreationProps {
    pub profile_name: Option<String>,
    pub fallback_profile_name: String,
    /// Only include the main thread of each process.
    #[allow(dead_code)]
    pub main_thread_only: bool,
    /// Merge non-overlapping threads of the same name.
    pub reuse_threads: bool,
    /// Fold repeated frames at the base of the stack.
    pub fold_recursive_prefix: bool,
    /// Unlink jitdump/marker files
    pub unlink_aux_files: bool,
    /// Create a separate thread for each CPU.
    pub create_per_cpu_threads: bool,
    /// Include up to N command line arguments in the process name
    pub arg_count_to_include_in_process_name: usize,
    /// Override system architecture.
    #[allow(dead_code)]
    pub override_arch: Option<String>,
    /// Dump presymbolication info.
    pub unstable_presymbolicate: bool,
    /// CoreCLR specific properties.
    #[allow(dead_code)]
    pub coreclr: CoreClrProfileProps,
    /// Create markers for unknown events.
    #[allow(dead_code)]
    pub unknown_event_markers: bool,
    /// Time range to include, relative to start of recording.
    #[allow(dead_code)]
    pub time_range: Option<(std::time::Duration, std::time::Duration)>,
}

impl ProfileCreationProps {
    pub fn profile_name(&self) -> &str {
        self.profile_name
            .as_deref()
            .unwrap_or(&self.fallback_profile_name)
    }
}

/// Properties which are meaningful for launching and recording a fresh process.
#[derive(Debug, Clone)]
pub struct ProcessLaunchProps {
    pub env_vars: Vec<(OsString, OsString)>,
    pub command_name: OsString,
    pub args: Vec<OsString>,
    pub iteration_count: u32,
}
