use std::path::{Path, PathBuf};

use fxprof_processed_profile::{Profile, ReferenceTimestamp, SamplingInterval};

use super::etw_gecko;
use crate::shared::included_processes::IncludedProcesses;
use crate::shared::prop_types::{ImportProps, ProfileCreationProps};
use crate::windows::profile_context::ProfileContext;

pub fn convert_etl_file_to_profile(filename: &Path, import_props: ImportProps) -> Profile {
    let timebase = std::time::SystemTime::now();
    let timebase = ReferenceTimestamp::from_system_time(timebase);

    let interval_8khz = SamplingInterval::from_nanos(122100); // 8192Hz // only with the higher recording rate?
    let profile = Profile::new(
        import_props.profile_creation_props.profile_name(),
        timebase,
        interval_8khz,
    );

    let arch = get_native_arch(); // TODO: Detect arch from file

    eprintln!("Processing ETL trace...");

    let mut context = ProfileContext::new(
        profile,
        arch,
        import_props.included_processes,
        import_props.profile_creation_props,
        import_props.time_range,
    );

    etw_gecko::process_etl_files(&mut context, filename, &import_props.user_etl);

    context.finish()
}

#[cfg(target_arch = "x86")]
fn get_native_arch() -> &'static str {
    "x86"
}

#[cfg(target_arch = "x86_64")]
fn get_native_arch() -> &'static str {
    "x86_64"
}

#[cfg(target_arch = "aarch64")]
fn get_native_arch() -> &'static str {
    "arm64"
}
