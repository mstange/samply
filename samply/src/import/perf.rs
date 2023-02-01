use framehop::{Module, Unwinder};
use fxprof_processed_profile::Profile;
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{PerfFileReader, PerfFileRecord};
use linux_perf_event_reader::EventRecord;

use std::io::{Read, Seek};
use std::path::Path;

use crate::linux_shared::{
    ConvertRegs, ConvertRegsAarch64, ConvertRegsX86_64, Converter, EventInterpretation,
};

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
    let build_ids = perf_file.build_ids().ok().unwrap_or_default();
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
                println!(
                    "bad timestamp ordering; {timestamp} is earlier but arrived after {last_timestamp}"
                );
            }
            last_timestamp = timestamp;
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
                converter.handle_mmap(e);
            }
            EventRecord::Mmap2(e) => {
                converter.handle_mmap2(e);
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
