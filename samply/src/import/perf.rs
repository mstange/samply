use framehop::{Module, Unwinder};
use fxprof_processed_profile::{LibraryHandle, Profile};
use linux_perf_data::jitdump::{JitDumpReader, JitDumpRecord, JitDumpRecordType};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{DsoInfo, DsoKey, PerfFileReader, PerfFileRecord};
use linux_perf_event_reader::EventRecord;

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use crate::linux_shared::{
    ConvertRegs, ConvertRegsAarch64, ConvertRegsX86_64, Converter, EventInterpretation,
};
use crate::utils::open_file_with_fallback;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Linux Perf error: {0}")]
    LinuxPerf(#[from] linux_perf_data::Error),
}

pub fn convert<C: Read + Seek>(cursor: C, extra_dir: Option<&Path>) -> Result<Profile, Error> {
    let perf_file = PerfFileReader::parse_file(cursor)?;

    let arch = perf_file.perf_file.arch().ok().flatten();

    let profile = match arch {
        Some("aarch64") => {
            let cache = framehop::aarch64::CacheAarch64::new();
            convert_impl::<framehop::aarch64::UnwinderAarch64<Vec<u8>>, ConvertRegsAarch64, _>(
                perf_file, extra_dir, cache,
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
            convert_impl::<framehop::x86_64::UnwinderX86_64<Vec<u8>>, ConvertRegsX86_64, _>(
                perf_file, extra_dir, cache,
            )
        }
    };
    Ok(profile)
}

fn convert_impl<U, C, R>(
    file: PerfFileReader<R>,
    extra_dir: Option<&Path>,
    cache: U::Cache,
) -> Profile
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
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
    let little_endian = perf_file.endian() == linux_perf_data::Endianness::LittleEndian;
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
    for event_name in attributes.iter().filter_map(|attr| attr.name()) {
        println!("event {event_name}");
    }
    let interpretation = EventInterpretation::divine_from_attrs(attributes);

    let product = "Converted perf profile";
    let mut converter = Converter::<U>::new(
        product,
        Some(Box::new(move |name| {
            format!("{name} on {host} (perf version {perf_version})")
        })),
        build_ids,
        linux_version,
        first_sample_time,
        little_endian,
        cache,
        extra_dir,
        interpretation.clone(),
    );

    let mut last_timestamp = 0;
    let mut jitdumps: Vec<JitDump> = Vec::new();

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
        for jitdump in &mut jitdumps {
            while let Ok(Some(next_record_header)) = jitdump.reader.next_record_header() {
                if next_record_header.timestamp > last_timestamp {
                    break;
                }
                match next_record_header.record_type {
                    JitDumpRecordType::JIT_CODE_LOAD | JitDumpRecordType::JIT_CODE_MOVE => {
                        // These are interesting.
                    }
                    _ => {
                        // We skip other records. We especially want to skip JIT_CODE_DEBUG_INFO
                        // records because they can be big and we don't need to read them from
                        // the file.
                        if let Ok(true) = jitdump.reader.skip_next_record() {
                            continue;
                        } else {
                            break;
                        }
                    }
                }
                let Ok(Some(raw_jitdump_record)) = jitdump.reader.next_record() else { break };
                match raw_jitdump_record.parse() {
                    Ok(JitDumpRecord::CodeLoad(code_load_record)) => {
                        converter.handle_jit_code_load(
                            raw_jitdump_record.start_offset,
                            raw_jitdump_record.timestamp,
                            jitdump.lib_handle,
                            &code_load_record,
                        );
                    }
                    Ok(JitDumpRecord::CodeMove(code_move_record)) => {
                        converter.handle_jit_code_move(
                            raw_jitdump_record.timestamp,
                            jitdump.lib_handle,
                            &code_move_record,
                        );
                    }
                    _ => {}
                }
            }
        }
        match parsed_record {
            EventRecord::Sample(e) => {
                if attr_index == interpretation.main_event_attr_index {
                    converter.handle_sample::<C>(e);
                } else if interpretation.sched_switch_attr_index == Some(attr_index) {
                    converter.handle_sched_switch::<C>(e);
                }
            }
            EventRecord::Fork(e) => {
                converter.handle_thread_start(e);
            }
            EventRecord::Comm(e) => {
                converter.handle_thread_name_update(e, record.timestamp());
            }
            EventRecord::Exit(e) => {
                converter.handle_thread_end(e);
            }
            EventRecord::Mmap(e) => {
                if let Some((reader, path)) = load_jitdump(&e.path.as_slice(), extra_dir) {
                    let lib_handle = converter.lib_handle_for_jitdump(&path, reader.header());
                    jitdumps.push(JitDump { reader, lib_handle });
                } else {
                    converter.handle_mmap(e, last_timestamp);
                }
            }
            EventRecord::Mmap2(e) => {
                if let Some((reader, path)) = load_jitdump(&e.path.as_slice(), extra_dir) {
                    let lib_handle = converter.lib_handle_for_jitdump(&path, reader.header());
                    jitdumps.push(JitDump { reader, lib_handle });
                } else {
                    converter.handle_mmap2(e, last_timestamp);
                }
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

struct JitDump {
    reader: JitDumpReader<std::fs::File>,
    lib_handle: LibraryHandle,
}

fn load_jitdump(
    path: &[u8],
    extra_dir: Option<&Path>,
) -> Option<(JitDumpReader<std::fs::File>, PathBuf)> {
    let jitdump_path = get_path_if_jitdump(path)?;
    let (file, actual_path) = match open_file_with_fallback(jitdump_path, extra_dir) {
        Ok(file_and_path) => file_and_path,
        Err(e) => {
            eprintln!("Could not open JITDUMP file at {jitdump_path:?}: {e}");
            return None;
        }
    };
    let reader = match JitDumpReader::new(file) {
        Ok(reader) => reader,
        Err(e) => {
            eprintln!("Could not parse JITDUMP file at {actual_path:?}: {e}");
            return None;
        }
    };

    Some((reader, actual_path))
}

fn get_path_if_jitdump(path: &[u8]) -> Option<&Path> {
    let path = Path::new(std::str::from_utf8(path).ok()?);
    let filename = path.file_name()?.to_str()?;
    if filename.starts_with("jit-") && filename.ends_with(".dump") {
        Some(path)
    } else {
        None
    }
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
