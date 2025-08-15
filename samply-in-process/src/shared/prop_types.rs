use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use serde_derive::{Deserialize, Serialize};

use super::included_processes::IncludedProcesses;

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
    pub time_limit: Option<Duration>,
    pub interval: Duration,
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
    /// Include up to N command line arguments in the process name
    pub arg_count_to_include_in_process_name: usize,

    pub unlink_aux_files: bool,
    pub should_emit_jit_markers: bool,
    pub fold_recursive_prefix: bool,
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
    pub ignore_exit_code: bool,
}

#[derive(Debug, Clone)]
pub struct ImportProps {
    pub profile_creation_props: ProfileCreationProps,
    pub symbol_props: SymbolProps,
    pub aux_file_dir: Vec<PathBuf>,
    #[allow(unused)] // todo: respect when converting perf.data
    pub included_processes: Option<IncludedProcesses>,
    #[allow(unused)] // Windows-only
    pub user_etl: Vec<PathBuf>,
    #[allow(unused)] // todo: respect when converting perf.data
    pub time_range: Option<(std::time::Duration, std::time::Duration)>,
}

#[derive(Debug, Clone)]
pub struct SymbolProps {
    /// Extra directories containing symbol files
    pub symbol_dir: Vec<PathBuf>,
    /// Additional URLs of symbol servers serving PDB / DLL / EXE files
    pub windows_symbol_server: Vec<String>,
    /// Overrides the default cache directory for Windows symbol files which were downloaded from a symbol server
    pub windows_symbol_cache: Option<PathBuf>,
    /// Additional URLs of symbol servers serving Breakpad .sym files
    pub breakpad_symbol_server: Vec<String>,
    /// Additional local directories containing Breakpad .sym files
    pub breakpad_symbol_dir: Vec<String>,
    /// Overrides the default cache directory for Breakpad symbol files
    pub breakpad_symbol_cache: Option<PathBuf>,
    /// Extra directory containing symbol files, with the directory structure used by simpleperf's scripts
    pub simpleperf_binary_cache: Option<PathBuf>,
}
