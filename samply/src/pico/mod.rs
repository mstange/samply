#![allow(unused)]
use crate::server::{start_server_main, ServerProps};
use crate::shared::ctrl_c::CtrlC;
use crate::shared::recording_props::{self, RecordingProps};
use crate::shared::symbol_props::SymbolProps;
use crate::shared::{
    lib_mappings::LibMappingOpQueue,
    process_sample_data::ProcessSampleData,
    recording_props::ProfileCreationProps,
    timestamp_converter::TimestampConverter,
    types::{StackFrame, StackMode},
    unresolved_samples::{UnresolvedSamples, UnresolvedStacks},
};
use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryColor, CpuDelta, LibraryInfo, Profile, ReferenceTimestamp, SamplingInterval, Symbol, SymbolTable
};
use serde_json::to_writer;
use serialport5::SerialPort;
use wholesym::samply_symbols::debug_id_for_object;
use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, io::Write, path::Path, time::Duration};

const BOOTROM_LO: u64 = 0x0u64;
const BOOTROM_HI: u64 = 0x4000u64;
const FLASH_LO: u64 = 0x10000000u64;
const FLASH_HI: u64 = 0x10200000u64;
const MEM_LO  : u64 = 0x20000000u64;
const MEM_HI  : u64 = 0x20042000u64;

const START_SAMPLING_CMD: u8 = 0xa0;
const STOP_SAMPLING_CMD : u8 = 0xa1;
const SAMPLE_CMD        : u8 = 0xb0;

pub struct PicoProps {
    pub elf: String,
    pub serial: String,
    pub bootrom_elf: Option<String>,
    pub reset: bool,
}

fn to_stack_frames(addresses: &[u32]) -> Vec<StackFrame> {
    if addresses.is_empty() {
        return Vec::new();
    }

    let mut address_iter = addresses.iter();
    let first_addr = address_iter.next().unwrap();
    let mut frames = vec![StackFrame::InstructionPointer(
        *first_addr as u64,
        StackMode::User,
    )];
    // Note AdjustedReturnAddress below -- we expect the profiler
    // to take care of knowing whether we're in Thumb or ARM mode and to
    // subtract 2 or 4 from LR
    frames
        .extend(address_iter.map(|addr| StackFrame::AdjustedReturnAddress(*addr as u64, StackMode::User)));
    frames
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Object error {0}")]
    Object(#[from] object::read::Error),
}

pub(crate) fn record_pico(
    pico_props: PicoProps,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    symbol_props: SymbolProps,
    server_props: Option<ServerProps>,
) -> Profile {
    let elf_file_str = pico_props.elf.clone();
    let elf_file = PathBuf::from(pico_props.elf);
    let elf_filename_str = elf_file.file_name().unwrap().to_string_lossy().to_string();

    let debug_id = {
        let data = fs::read(elf_file).unwrap();
        let elf = object::File::parse(&*data).unwrap();

        debug_id_for_object(&elf).unwrap()
    };

    let bootrom_debug_id = if let Some(bootrom_elf) = &pico_props.bootrom_elf {
        let data = fs::read(bootrom_elf).unwrap();
        let elf = object::File::parse(&*data).unwrap();

        debug_id_for_object(&elf).unwrap()
    } else {
        DebugId::nil()
    };

    let sampling_interval_ns = recording_props.interval.as_nanos() as u64;

    let timestamp_converter = TimestampConverter {
        reference_raw: 0,
        raw_to_ns_factor: 1000, // times from platform are all us
    };

    let mut profile = Profile::new(
        &elf_filename_str,
        ReferenceTimestamp::from_millis_since_unix_epoch(0.),
        SamplingInterval::from_nanos(sampling_interval_ns),
    );

    let zero_time = timestamp_converter.convert_time(0);

    let process = profile.add_process(&elf_filename_str, 0, zero_time);
    let threads = vec![
        profile.add_thread(process, 0, zero_time, true),
        profile.add_thread(process, 1, zero_time, false),
    ];

    let main_exe_info = LibraryInfo {
        name: elf_filename_str.clone(),
        path: elf_file_str.clone(),
        debug_name: elf_filename_str.clone(),
        debug_path: elf_file_str.clone(),
        debug_id: debug_id,
        code_id: None,
        arch: Some("arm32".to_string()),
        symbol_table: None,
    };
    let main_exe = profile.add_lib(main_exe_info);

    let bootrom_symbol_table = if pico_props.bootrom_elf.is_some() {
        None // load it from the elf file dynamically
    } else {
        let symbol_table = SymbolTable::new(
            vec![Symbol { address: BOOTROM_LO as u32, size: Some((BOOTROM_HI - BOOTROM_LO) as u32), name: "bootrom".to_owned() }]
        );
        Some(Arc::new(symbol_table))
    };
    let bootrom_elf = PathBuf::from(pico_props.bootrom_elf.clone().unwrap_or_else(|| "bootrom.elf".to_string()));
    let bootrom_info = LibraryInfo {
        name: "bootrom".to_string(),
        path: bootrom_elf.to_string_lossy().to_string(),
        debug_name: bootrom_elf.file_name().unwrap().to_string_lossy().to_string(),
        debug_path: bootrom_elf.to_string_lossy().to_string(),
        debug_id: bootrom_debug_id,
        code_id: None,
        arch: Some("arm32".to_string()),
        symbol_table: bootrom_symbol_table,
    };
    let bootrom = profile.add_lib(bootrom_info);

    profile.add_lib_mapping(process, bootrom, BOOTROM_LO, BOOTROM_HI, 0);
    profile.add_lib_mapping(process, main_exe, FLASH_LO, MEM_HI, 0);

    let mut unresolved_stacks = UnresolvedStacks::default();
    let mut unresolved_samples = UnresolvedSamples::default();
    let mut regular_lib_mapping_ops = LibMappingOpQueue::default();

    const SERIAL_BUF_SIZE: usize = 32768;
    let mut serial_buf: Vec<u8> = vec![0; SERIAL_BUF_SIZE];
    let mut remainder: usize = 0;

    let mut ctrl_c_receiver = CtrlC::observe_oneshot();

    // TODO at some point handle wraparound
    let mut timestamp_base = 0u64;
    let mut timestamp_offset = 0u64;
    let mut last_timestamp_us = 0u64;

    let mut sample_core = 0;
    let mut sample_timestamp_us = 0u64;
    let mut sample_stack: Vec<u32> = Vec::new();
    let mut sample_stack_index = 0;
    let mut sample_stack_total = 0;

    eprintln!("Opening {}...", pico_props.serial);
    let mut port = SerialPort::builder()
        .baud_rate(460800)
        .read_timeout(Some(Duration::from_millis(10)))
        .open(PathBuf::from(pico_props.serial))
        .expect("Failed to open port");

    if pico_props.reset {
        eprintln!("Resetting target...");
        port.write(&[b'r']).expect("Failed to write reset cmd");
    }

    eprintln!("Starting profile...");

    {
        let sampling_interval_ms = (sampling_interval_ns / 1_000_000) as u16;
        let start_cmd = vec![START_SAMPLING_CMD,
            (sampling_interval_ms & 0xff) as u8,
            ((sampling_interval_ms >> 8) & 0xff) as u8,
            0];
        port.write(&start_cmd).expect("Failed to write start cmd");
    }

    eprintln!("Profiling. Press ^C to stop.");

    loop {
        if ctrl_c_receiver.try_recv().is_ok() {
            eprintln!("Stopping.");
            let _ = port.write(&[STOP_SAMPLING_CMD]).map_err(|e| eprintln!("Warning: failed to write stop command: {}", e));
            break;
        }

        let Ok(len) = port.read(&mut serial_buf[remainder..]) else {
            continue;
        };

        if len == 0 {
            continue;
        }

        let len = len + remainder;
        let extra = len % 4;

        //eprintln!("len {} extra {}", len, extra);

        // format:
        //   4 bytes: 0xbq /* where q == core_num */ | (num_frames & 0xfff), other bits reserved
        //   4 bytes: u32 timestamp (ns; may loop around)
        //   ... number of frames * 4 bytes ... (u32 addresses)
        for value in serial_buf[..len - extra]
            .chunks_exact(4)
            .map(|a| u32::from_le_bytes(a.try_into().unwrap()))
        {
            if sample_stack_index == u32::MAX {
                // timestamp
                sample_timestamp_us = value as u64;
                if timestamp_base == 0 {
                    timestamp_base = sample_timestamp_us;
                }

                sample_stack_index = 0;

                if last_timestamp_us > sample_timestamp_us {
                    // wrapped around
                    timestamp_offset += 0x1_0000_0000u64;
                }

                sample_timestamp_us += timestamp_offset;
                last_timestamp_us = sample_timestamp_us;
                //eprintln!("sample_timestamp_us {}", sample_timestamp_us);
            } else if sample_stack_index < sample_stack_total {
                //eprintln!("sample 0x{:08x}", value);
                sample_stack.push(value);
                sample_stack_index += 1;

                if sample_stack_index == sample_stack_total {
                    // we just finished a sample; record it in the profile
                    //eprintln!("Sample: core={}, timestamp={}, stack={:?}", sample_core, sample_timestamp_us, sample_stack);

                    let timestamp =
                        timestamp_converter.convert_time(sample_timestamp_us - timestamp_base);

                    // The stack we have starts with pc as the first element, and then each caller going back towards
                    // the entry fn.
                    let stack = to_stack_frames(&sample_stack);

                    // unresolved_stacks.convert() wants the opposite order (entry fn first, then callee-most), so that
                    // it can see common/shared stack prefixes first
                    let stack_index = unresolved_stacks.convert(stack.into_iter().rev());

                    unresolved_samples.add_sample(
                        threads[sample_core as usize],
                        timestamp,
                        sample_timestamp_us,
                        stack_index,
                        CpuDelta::from_nanos(sampling_interval_ns),
                        1,
                        None,
                    );
                    sample_stack_total = 0;
                }
            } else {
                // start of the next sample (sample_stack_index == sample_stack_total == 0, at init)
                let cmd = ((value >> 24) & 0xff) as u8;
                assert_eq!(cmd & 0xf0, SAMPLE_CMD);
                sample_core = cmd & 1;

                // start of the next sample, parse the header
                sample_stack_index = u32::MAX;
                sample_stack_total = value & 0xfff;
                sample_stack.clear();
                //eprintln!("sample start {} samples", sample_stack_total);
            }
        }

        if extra > 0 {
            if extra == 3 { serial_buf[0] = serial_buf[len - 3]; }
            if extra >= 2 { serial_buf[1] = serial_buf[len - 2]; }
            if extra >= 1 { serial_buf[2] = serial_buf[len - 1]; }
        }

        remainder = extra;
    }

    let process_sample_data = ProcessSampleData::new(
        unresolved_samples,
        regular_lib_mapping_ops,
        vec![],
        None,
        Vec::new(),
    );

    let user_category = profile.add_category("User", CategoryColor::Yellow);

    let mut stack_frame_scratch_buf = Vec::new();
    process_sample_data.flush_samples_to_profile(
        &mut profile,
        user_category.into(),
        user_category.into(),
        &mut stack_frame_scratch_buf,
        &unresolved_stacks,
    );

    let output_file = recording_props.output_file.clone();
    if profile_creation_props.unstable_presymbolicate {
        crate::shared::symbol_precog::presymbolicate(
            &profile,
            &output_file.with_extension("syms.json"),
        );
    }

    // write the profile to a json file
    let file = File::create(&output_file).unwrap();
    let writer = BufWriter::new(file);
    {
        to_writer(writer, &profile).expect("Couldn't write JSON");
    }

    // then fire up the server for the profiler front end, if not save-only
    if let Some(server_props) = server_props {
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(&output_file).expect("Couldn't open file we just wrote"),
            &output_file,
        )
        .expect("Couldn't parse libinfo map from profile file");

        start_server_main(&output_file, server_props, symbol_props, libinfo_map);
    }

    profile
}
