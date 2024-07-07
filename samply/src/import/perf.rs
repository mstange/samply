use std::collections::HashMap;
use std::fmt::Write;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::time::SystemTime;

use framehop::{Module, Unwinder};
use fxprof_processed_profile::{Profile, ReferenceTimestamp};
use linux_perf_data::{linux_perf_event_reader, DsoInfo, DsoKey, PerfFileReader, PerfFileRecord};
use linux_perf_event_reader::EventRecord;

use crate::linux_shared::{
    ConvertRegs, ConvertRegsAarch64, ConvertRegsX86_64, Converter, EventInterpretation, KnownEvent,
    MmapRangeOrVec,
};
use crate::shared::recording_props::ProfileCreationProps;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Linux Perf error: {0}")]
    LinuxPerf(#[from] linux_perf_data::Error),
}

pub fn convert<C: Read + Seek>(
    cursor: C,
    file_mod_time: Option<SystemTime>,
    binary_lookup_dirs: Vec<PathBuf>,
    aux_file_lookup_dirs: Vec<PathBuf>,
    profile_creation_props: ProfileCreationProps,
) -> Result<Profile, Error> {
    let perf_file = PerfFileReader::parse_file(cursor)?;

    let arch = perf_file.perf_file.arch().ok().flatten();

    let profile = match arch {
        Some("aarch64") => {
            let cache = framehop::aarch64::CacheAarch64::new();
            convert_impl::<framehop::aarch64::UnwinderAarch64<MmapRangeOrVec>, ConvertRegsAarch64, _>(
                perf_file,
                file_mod_time,
                binary_lookup_dirs,
                aux_file_lookup_dirs,
                cache,
                profile_creation_props,
            )
        }
        _ => {
            if arch != Some("x86_64") {
                eprintln!(
                    "Unknown arch {}, dwarf-based unwinding may be incorrect.",
                    arch.unwrap_or_default()
                );
            }
            let cache = framehop::x86_64::CacheX86_64::new();
            convert_impl::<framehop::x86_64::UnwinderX86_64<MmapRangeOrVec>, ConvertRegsX86_64, _>(
                perf_file,
                file_mod_time,
                binary_lookup_dirs,
                aux_file_lookup_dirs,
                cache,
                profile_creation_props,
            )
        }
    };
    Ok(profile)
}

fn convert_impl<U, C, R>(
    file: PerfFileReader<R>,
    file_mod_time: Option<SystemTime>,
    binary_lookup_dirs: Vec<PathBuf>,
    aux_file_lookup_dirs: Vec<PathBuf>,
    cache: U::Cache,
    profile_creation_props: ProfileCreationProps,
) -> Profile
where
    U: Unwinder<Module = Module<MmapRangeOrVec>> + Default,
    C: ConvertRegs<UnwindRegs = U::UnwindRegs>,
    R: Read,
{
    let PerfFileReader {
        mut perf_file,
        mut record_iter,
    } = file;
    let mut build_ids = perf_file.build_ids().ok().unwrap_or_default();
    fixup_perf_jit_build_ids(&mut build_ids);
    let first_sample_time = perf_file
        .sample_time_range()
        .unwrap()
        .map_or(0, |r| r.first_sample_time);
    let endian = perf_file.endian();
    let simpleperf_meta_info = perf_file.simpleperf_meta_info().ok().flatten();
    let is_simpleperf = simpleperf_meta_info.is_some();
    let call_chain_return_addresses_are_preadjusted = is_simpleperf;

    let linux_version = perf_file.os_release().unwrap();
    let attributes = perf_file.event_attributes();
    if let Ok(Some(cmd_line)) = perf_file.cmdline() {
        eprintln!("cmd line: {}", cmd_line.join(" "));
    }
    for event_name in attributes.iter().filter_map(|attr| attr.name()) {
        eprintln!("event {event_name}");
    }
    let interpretation = EventInterpretation::divine_from_attrs(attributes);
    let simpleperf_symbol_tables = perf_file.simpleperf_symbol_tables().ok().flatten();
    let reference_timestamp = if let Some(seconds_since_unix_epoch) =
        get_simpleperf_timestamp(simpleperf_meta_info.as_ref())
    {
        ReferenceTimestamp::from_millis_since_unix_epoch(seconds_since_unix_epoch * 1000.0)
    } else if let Some(mod_time) = file_mod_time {
        ReferenceTimestamp::from_system_time(mod_time)
    } else {
        ReferenceTimestamp::from_system_time(SystemTime::now())
    };

    let (profile_name, mut profile_name_postfix_for_first_process) = if let Some(profile_name) =
        profile_creation_props.profile_name.clone()
    {
        // The user gave us an explicit profile name. Use it.
        (profile_name, None)
    } else if let Some(simpleperf_meta_info) = simpleperf_meta_info.as_ref() {
        // perf.data from simpleperf
        let mut profile_name_postfix = String::new();
        if let Some(profile_name_props) = simpleperf_meta_info.get("product_props") {
            // Example: "Google:Pixel 6:oriole"
            let fragments: Vec<&str> = profile_name_props.split(':').take(2).collect();
            if !fragments.is_empty() {
                let device_name = fragments.join(" ");
                write!(profile_name_postfix, " on {device_name}").unwrap();
            }
        }
        if let Some(app_package_name) = simpleperf_meta_info.get("app_package_name") {
            // We were profiling a single app.
            (format!("{app_package_name}{profile_name_postfix}"), None)
        } else {
            // We would like the profile name to start with the name of the process / app
            // that has been profiled. However, we don't know this name yet.
            // Start with the name of the imported perf.data file, but also store the
            // profile name "postfix" so that we can change the profile name later, once
            // we see the first profiled process.
            let imported_file_filename = profile_creation_props.fallback_profile_name.clone();
            let initial_profile_name = format!("{imported_file_filename}{profile_name_postfix}");
            (initial_profile_name, Some(profile_name_postfix))
        }
    } else {
        // perf.data from Linux perf
        let mut profile_name_postfix = String::new();
        if let Some(host) = perf_file.hostname().ok().flatten() {
            write!(profile_name_postfix, " on {host}").unwrap();
        }
        if let Some(perf_version) = perf_file.perf_version().ok().flatten() {
            write!(profile_name_postfix, " (perf version {perf_version})").unwrap();
        }
        // We would like the profile name to start with the name of the process / executable
        // that has been profiled. However, we don't know this name yet.
        // Start with the name of the imported perf.data file, but also store the
        // profile name "postfix" so that we can change the profile name later, once
        // we see the first profiled process.
        let imported_file_filename = profile_creation_props.fallback_profile_name.clone();
        let initial_profile_name = format!("{imported_file_filename}{profile_name_postfix}");
        (initial_profile_name, Some(profile_name_postfix))
    };

    let mut converter = Converter::<U>::new(
        &profile_creation_props,
        reference_timestamp,
        &profile_name,
        build_ids,
        linux_version,
        first_sample_time,
        endian,
        cache,
        binary_lookup_dirs,
        aux_file_lookup_dirs,
        interpretation.clone(),
        simpleperf_symbol_tables,
        call_chain_return_addresses_are_preadjusted,
    );

    if let Some(android_version) = simpleperf_meta_info
        .as_ref()
        .and_then(|mi| mi.get("android_version"))
    {
        converter.set_os_name(&format!("Android {android_version}"));
    }

    let mut last_timestamp = 0;

    while let Ok(Some(record)) = record_iter.next_record(&mut perf_file) {
        let (record, parsed_record, attr_index) = match record {
            PerfFileRecord::EventRecord { attr_index, record } => match record.parse() {
                Ok(r) => (record, r, attr_index),
                Err(_) => continue,
            },
            PerfFileRecord::UserRecord(_) => continue,
        };
        if let Some(timestamp) = record.timestamp() {
            if timestamp < last_timestamp {
                eprintln!(
                    "bad timestamp ordering; {timestamp} is earlier but arrived after {last_timestamp}"
                );
            }
            last_timestamp = timestamp;
        }

        match parsed_record {
            EventRecord::Sample(e) => {
                if attr_index == interpretation.main_event_attr_index {
                    converter.handle_main_event_sample::<C>(&e);
                } else if Some(attr_index) == interpretation.sched_switch_attr_index {
                    converter.handle_sched_switch_sample::<C>(&e);
                }

                match interpretation.known_event_indices.get(&attr_index) {
                    Some(KnownEvent::RssStat) => converter.handle_rss_stat_sample::<C>(&e),
                    _ => {
                        // the main event and sched_switch are already covered by regular samples so don't add other event markers
                        if !(attr_index == interpretation.main_event_attr_index
                            || Some(attr_index) == interpretation.sched_switch_attr_index)
                        {
                            converter.handle_other_event_sample::<C>(&e, attr_index)
                        }
                    }
                }
            }
            EventRecord::Fork(e) => {
                converter.handle_fork(e);
            }
            EventRecord::Comm(e) => {
                if profile_name_postfix_for_first_process.is_some()
                    && &e.name.as_slice()[..] != b"perf-exec"
                {
                    let postfix = profile_name_postfix_for_first_process.take().unwrap();
                    let first_process_name =
                        String::from_utf8_lossy(&e.name.as_slice()).to_string();
                    let profile_name = format!("{first_process_name}{postfix}");
                    converter.set_profile_name(&profile_name);
                }
                converter.handle_comm(e, record.timestamp());
            }
            EventRecord::Exit(e) => {
                converter.handle_exit(e);
            }
            EventRecord::Mmap(e) => {
                converter.handle_mmap(e, last_timestamp);
            }
            EventRecord::Mmap2(e) => {
                converter.handle_mmap2(e, last_timestamp);
            }
            EventRecord::ContextSwitch(e) => {
                let common = match record.common_data() {
                    Ok(common) => common,
                    Err(_) => continue,
                };
                converter.handle_context_switch(e, common);
            }
            _ => {
                // println!("{:?}", record.record_type);
            }
        }
    }

    converter.finish()
}

fn get_simpleperf_timestamp(meta_info: Option<&HashMap<&str, &str>>) -> Option<f64> {
    let meta_info = meta_info?;
    let timestamp_str = meta_info.get("timestamp")?;
    timestamp_str.parse().ok()
}

/// This is a terrible hack to work around ambiguous build IDs in old versions
/// of perf (tested with perf 5.4.224). Those versions of perf do two things:
///
///  - They do not write down the length of the build ID in the perf.data file.
///  - They generate .so files with 20-byte build IDs which end in [..., 0, 0, 0, 0].
///
/// The former means that linux-perf-data needs to guess the build ID length,
/// which it does by stripping off 4-byte chunks of zeros from the end.
///
/// The latter means that the guessed build ID length for these JIT images is 16, when
/// it really should have been 20 (including the 4 zeros at the end). So we correct
/// that guess here, based on the file name.
///
/// Some ELF files legitimately have a build ID of 16 bytes, so we must make sure
/// to leave those alone.
///
/// TODO: We should not do this adjustment if the length of 16 was written down in
/// the perf.data file. However, at the moment linux-perf-data doesn't tell us
/// whether the build ID length is "real" or guessed.
fn fixup_perf_jit_build_ids(build_ids: &mut HashMap<DsoKey, DsoInfo>) {
    for (key, info) in build_ids {
        let name = key.name();
        if name.starts_with("jitted-") && name.ends_with(".so") && info.build_id.len() == 16 {
            // Extend to 20 bytes.
            info.build_id.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
}
