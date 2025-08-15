#[cfg(target_os = "macos")]
mod mac;
mod shared;

use std::{path::Path, time::Duration};

use shared::save_profile::save_profile_to_file;

#[cfg(target_os = "macos")]
pub use crate::{
    mac::profiler::RunningProfiler,
};
use crate::{
    shared::prop_types::{ProfileCreationProps, RecordingProps},
};

pub fn start_profiling(interval: Duration
) -> RunningProfiler {
    let recording_props = RecordingProps {
        time_limit: None,
        interval,
    };
    let profile_creation_props = ProfileCreationProps {
        profile_name: None,
        fallback_profile_name: "samply-in-process profile".to_string(),
        main_thread_only: true,
        reuse_threads: false,
        unlink_aux_files: false,
        should_emit_jit_markers: false,
        fold_recursive_prefix: false,
        arg_count_to_include_in_process_name: 100,
    };
    RunningProfiler::start_recording(recording_props, profile_creation_props)
}

pub fn stop_profiling_and_save(running_profiler: RunningProfiler, output: &Path) {
    let profile = running_profiler.stop_and_capture_profile().unwrap();
    save_profile_to_file(&profile, output).expect("Couldn't write JSON");
}
