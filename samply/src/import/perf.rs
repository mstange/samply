use framehop::{Module, Unwinder};
use fxprof_processed_profile::Profile;
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{DsoInfo, DsoKey, PerfFileReader, PerfFileRecord};
use linux_perf_event_reader::EventRecord;

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

use crate::linux_shared::{
    ConvertRegs, ConvertRegsAarch64, ConvertRegsX86_64, Converter, EventInterpretation, KnownEvent,
    MmapRangeOrVec,
};
use crate::shared::recording_props::ConversionProps;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Linux Perf error: {0}")]
    LinuxPerf(#[from] linux_perf_data::Error),
}

pub fn convert<C: Read + Seek>(
    cursor: C,
    extra_dir: Option<&Path>,
    conversion_props: ConversionProps,
) -> Result<Profile, Error> {
    let perf_file = PerfFileReader::parse_file(cursor)?;

    let arch = perf_file.perf_file.arch().ok().flatten();

    let profile = match arch {
        Some("aarch64") => {
            let cache = framehop::aarch64::CacheAarch64::new();
            convert_impl::<framehop::aarch64::UnwinderAarch64<MmapRangeOrVec>, ConvertRegsAarch64, _>(
                perf_file,
                extra_dir,
                cache,
                conversion_props,
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
                extra_dir,
                cache,
                conversion_props,
            )
        }
    };
    Ok(profile)
}

fn convert_impl<U, C, R>(
    file: PerfFileReader<R>,
    extra_dir: Option<&Path>,
    cache: U::Cache,
    conversion_props: ConversionProps,
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
    let host = perf_file
        .hostname()
        .unwrap()
        .unwrap_or("<unknown host>")
        .to_owned();
    let perf_version = perf_file
        .perf_version()
        .unwrap()
        .unwrap_or("<unknown version>")
        .to_owned();
    let linux_version = perf_file.os_release().unwrap();
    let attributes = perf_file.event_attributes();
    if let Ok(Some(cmd_line)) = perf_file.cmdline() {
        eprintln!("cmd line: {}", cmd_line.join(" "));
    }
    for event_name in attributes.iter().filter_map(|attr| attr.name()) {
        eprintln!("event {event_name}");
    }
    let interpretation = EventInterpretation::divine_from_attrs(attributes);

    let mut converter = Converter::<U>::new(
        &conversion_props.profile_name,
        Some(Box::new(move |name| {
            format!("{name} on {host} (perf version {perf_version})")
        })),
        build_ids,
        linux_version,
        first_sample_time,
        endian,
        cache,
        extra_dir,
        interpretation.clone(),
        conversion_props.merge_threads,
        conversion_props.fold_recursive_prefix,
    );

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
