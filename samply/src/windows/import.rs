use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use fxprof_processed_profile::{Profile, ReferenceTimestamp, SamplingInterval};
use serde_json::to_writer;

use super::etw_gecko;
use crate::shared::included_processes::IncludedProcesses;
use crate::shared::recording_props::ProfileCreationProps;
use crate::windows::profile_context::ProfileContext;

pub fn convert_etl_file_to_profile(
    filename: &Path,
    output_filename: &Path,
    profile_creation_props: ProfileCreationProps,
    included_processes: Option<IncludedProcesses>,
) {
    let timebase = std::time::SystemTime::now();
    let timebase = ReferenceTimestamp::from_system_time(timebase);

    let interval_8khz = SamplingInterval::from_nanos(122100); // 8192Hz // only with the higher recording rate?
    let profile = Profile::new(
        &profile_creation_props.profile_name,
        timebase,
        interval_8khz, // recording_props.interval.into(),
    );

    let arch = get_native_arch(); // TODO: Detect arch from file

    let mut context =
        ProfileContext::new(profile, arch, included_processes, profile_creation_props);

    etw_gecko::profile_pid_from_etl_file(&mut context, filename);

    let profile = context.finish();

    // write the profile to a json file
    let file = File::create(output_filename).unwrap();
    let writer = BufWriter::new(file);
    {
        to_writer(writer, &profile).expect("Couldn't write JSON");
    }
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
