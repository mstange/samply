#![allow(unused)]
use crate::server::{start_server_main, ServerProps};
use crate::shared::ctrl_c::CtrlC;
use crate::shared::recording_props::RecordingProps;
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
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol, SectionFlags, SectionKind, SegmentFlags};
use serde_json::to_writer;
use serialport5::SerialPort;
use wholesym::samply_symbols::debug_id_for_object;
use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::{fs, io::Write, path::Path, time::Duration};

use super::PicoProps;

const BOOTROM_LO: u64 = 0x0u64;
const BOOTROM_HI: u64 = 0x4000u64;
const FLASH_LO: u64 = 0x10000000u64;
const FLASH_HI: u64 = 0x10200000u64;
const MEM_LO  : u64 = 0x20000000u64;
const MEM_HI  : u64 = 0x20042000u64;

const START_SAMPLING_CMD: u8 = 0xa0;
const STOP_SAMPLING_CMD : u8 = 0xa1;
const SAMPLE_CMD        : u8 = 0xb0;
const FLAG_ONE_CORE     : u8 = 0x01;

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

// Little hacky trait so that we can have an "ignore writes" path for being
// able to read from a device or a file. Maybe there's a better way to do this.
// The methods aren't just read/write to avoid conflicts later on where we
// do want to use the normal Read/Write traits.
trait ReaderWriter {
    fn read_buf(&mut self, data: &mut [u8]) -> std::io::Result<usize>;
    fn write_buf(&mut self, data: &[u8]) -> std::io::Result<usize>;
}

impl<T: std::io::Read + std::io::Write> ReaderWriter for T {
    fn read_buf(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.read(data)
    }

    fn write_buf(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.write(data)
    }
}

struct IgnoreWrites<T: std::io::Read> {
    reader: Box<T>
}

impl<T: std::io::Read> IgnoreWrites<T> {
    pub fn new(reader: T) -> IgnoreWrites<T> {
        IgnoreWrites { reader: Box::new(reader) }
    }
}

impl<T: std::io::Read> ReaderWriter for IgnoreWrites<T> {
    fn read_buf(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(data)
    }

    fn write_buf(&mut self, data: &[u8]) -> std::io::Result<usize> {
        Ok(data.len())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Object error {0}")]
    Object(#[from] object::read::Error),
}

#[derive(Debug, Default)]
struct SegmentInfo {
    pub load_addr: u64,
    pub load_size: u64,
    pub phys_addr: u64,
    pub file_range: (u64, u64),
}

#[derive(Debug, Default)]
struct ElfInfo {
    pub debug_id: DebugId,
    pub segments: Vec<SegmentInfo>,
    pub load_addr: u64,
    pub symbols: Vec<Symbol>,
}

impl ElfInfo {
    pub fn from_file(elf_file: &str) -> ElfInfo {
        let data = fs::read(elf_file).unwrap();
        let elf = object::read::elf::ElfFile32::parse(&*data).unwrap();

        let segments: Vec<_> = elf.segments()
            .filter(|s| {
                match s.flags() {
                    SegmentFlags::Elf { p_flags } => { (p_flags & 0x1) == 0x1 }
                    _ => { false }
                }
            })
            .map(|s| {
                let p = s.elf_program_header();

                let address = p.p_vaddr.get(object::LittleEndian) as u64;
                let phys_address = p.p_paddr.get(object::LittleEndian) as u64;
                let size = p.p_memsz.get(object::LittleEndian) as u64;
                let offs = p.p_offset.get(object::LittleEndian) as u64;
                let filesz = p.p_filesz.get(object::LittleEndian) as u64;
                let file_range = (offs, offs + filesz);
                SegmentInfo {
                    load_addr: address,
                    load_size: size,
                    phys_addr: phys_address,
                    file_range,
                }
            })
            .collect();

        let symbols = elf.symbols()
        .filter(|elfsym| elfsym.name().is_ok() && elfsym.kind() == object::SymbolKind::Text)
        .map(|elfsym| {
            let sym = Symbol {
                address: if elfsym.address() > 0x1000_0000 { elfsym.address() as u32 - 0x1000_0000 } else { elfsym.address() as u32 },
                size: if elfsym.size() > 0  { Some(elfsym.size() as u32) } else { None },
                name: elfsym.name().unwrap().to_string()
            };
            //eprintln!("{:x?}", sym);
            sym
        }).collect();

        ElfInfo {
            debug_id: debug_id_for_object(&elf).unwrap(),
            load_addr: segments[0].load_addr,
            segments,
            symbols
        }
    }
}

pub(crate) fn record_pico(
    pico_props: PicoProps,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    symbol_props: SymbolProps,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, ()> {
    let elf_file_str = pico_props.elf.clone();
    let elf_file = PathBuf::from(pico_props.elf);
    let elf_filename_str = elf_file.file_name().unwrap().to_string_lossy().to_string();

    let elf_info = ElfInfo::from_file(&elf_file_str);
    let bootrom_info = pico_props.bootrom_elf
        .as_ref()
        .map(|f| ElfInfo::from_file(f))
        .unwrap_or_default();

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
        //name: elf_filename_str.clone(),
        //path: elf_file_str.clone(),
        //debug_name: elf_filename_str.clone(),
        //debug_path: elf_file_str.clone(),
        name: "the_firmware".to_owned(),
        path: "the_firmware".to_owned(),
        debug_name: "the_firmware".to_owned(),
        debug_path: "the_firmware".to_owned(),
        debug_id: elf_info.debug_id,
        code_id: None,
        arch: Some("arm32".to_string()),
        symbol_table: Some(Arc::new(SymbolTable::new(elf_info.symbols.clone()))),
    };
    let main_exe = profile.add_lib(main_exe_info);

    let bootrom_symbol_table = if pico_props.bootrom_elf.is_some() {
        Some(Arc::new(SymbolTable::new(bootrom_info.symbols.clone())))
    } else {
        let symbol_table = SymbolTable::new(
            vec![Symbol { address: BOOTROM_LO as u32, size: Some((BOOTROM_HI - BOOTROM_LO) as u32), name: "bootrom".to_owned() }]
        );
        Some(Arc::new(symbol_table))
    };
    let bootrom_path = pico_props.bootrom_elf.clone().unwrap_or_else(|| "bootrom".to_owned());
    let bootrom_lib_info = LibraryInfo {
        name: "bootrom".to_string(),
        path: bootrom_path.clone(),
        debug_name: PathBuf::from(&bootrom_path).file_name().unwrap().to_string_lossy().to_string(),
        debug_path: bootrom_path.clone(),
        debug_id: bootrom_info.debug_id,
        code_id: None,
        arch: Some("arm32".to_string()),
        symbol_table: bootrom_symbol_table,
    };
    let bootrom = profile.add_lib(bootrom_lib_info);

    if bootrom_info.segments.is_empty() {
        profile.add_lib_mapping(process, bootrom, BOOTROM_LO, BOOTROM_HI, 0);
    } else {
        let load_addr = bootrom_info.segments[0].load_addr;
        if load_addr != BOOTROM_LO {
            log::warn!("Warning: bootrom ELF file load address is {:x}, expected {:x}", load_addr, BOOTROM_LO);
        }
        for segment in &bootrom_info.segments {
            //eprintln!("Adding bootrom: {:x?}", segment);
            profile.add_lib_mapping(process, bootrom, segment.load_addr, segment.load_addr + segment.load_size,
                (segment.load_addr - load_addr) as u32);
        }
    }

    // Samply & the front end can't handle multiple executable LOAD segments in one file.
    // We just provide the symbols explicitly (hacked to get the correct RVA) for the main exe
    if true || elf_info.segments.is_empty() {
        //eprintln!("Warning: found no segment map info in ELF file, mapping entire flash and memory range");
        profile.add_lib_mapping(process, main_exe, FLASH_LO, MEM_HI, 0);
    } else {
        let load_addr = elf_info.segments[0].phys_addr;

        for segment in &elf_info.segments {
            //eprintln!("Adding {}: {:x?}", elf_filename_str, segment);
            profile.add_lib_mapping(process, main_exe, segment.load_addr, segment.load_addr + segment.load_size,
                 0 /*(segment.phys_addr - load_addr) as u32*/);
                break;
        }
    }

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

    let device_path = &pico_props.device;
    let is_special = device_path.starts_with("/dev") ||
        device_path.starts_with("/sys") ||
        device_path.starts_with("\\\\");

    let (mut device, mut save_file) = if is_special {
        eprintln!("Opening {}...", pico_props.device);
        let mut port = SerialPort::builder()
            .baud_rate(460800)
            .read_timeout(Some(Duration::from_millis(10)))
            .open(PathBuf::from(pico_props.device))
            .expect("Failed to open port");

        let save_file = pico_props.save_file.map(|f|
            File::create(f).expect("Failed to open output save file"));

        let device = Box::new(port) as Box<dyn ReaderWriter>;

        (device, save_file)
    } else {
        let mut file = File::open("profile.dump").expect("Failed to open profile.dump");
        (Box::new(IgnoreWrites::new(file)) as Box<dyn ReaderWriter>, None)
    };

    //let mut dump_file = File::create("profile.dump").expect("Failed to open profile.dump");

    if pico_props.reset {
        eprintln!("Resetting target...");
        device.write_buf(&[b'r']).expect("Failed to write reset cmd");
    }

    eprintln!("Starting profile...");

    {
        let sampling_interval_ms = (sampling_interval_ns / 1_000_000) as u16;
        let start_cmd = vec![START_SAMPLING_CMD,
            (sampling_interval_ms & 0xff) as u8,
            ((sampling_interval_ms >> 8) & 0xff) as u8,
            0x01 as u8];
        device.write_buf(&start_cmd).expect("Failed to write start cmd");
    }

    eprintln!("Profiling. Press ^C to stop.");

    loop {
        if ctrl_c_receiver.try_recv().is_ok() {
            eprintln!("Stopping.");
            let _ = device.write_buf(&[STOP_SAMPLING_CMD]).map_err(|e| eprintln!("Warning: failed to write stop command: {}", e));
            break;
        }

        let Ok(len) = device.read_buf(&mut serial_buf[remainder..]) else {
            continue;
        };

        if len == 0 {
            continue;
        }

        if let Some(mut save_file) = save_file.as_ref() {
            save_file.write(&serial_buf[remainder..remainder+len]);
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

                // Mask off the lowest bit, which indicates whether the jump is into
                // thumb code or not. On Cortex-M, everything except $pc will always
                // have this bit set because it's always thumb, and we jump to those
                // addresses via instructions.
                sample_stack.push(value & !1);
                sample_stack_index += 1;

                if sample_stack_index == sample_stack_total {
                    // we just finished a sample; record it in the profile
                    //if !(sample_stack.len() == 2 && sample_stack[0] < 0x200) { eprintln!("{:x?}", sample_stack); }

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
                //eprintln!("cmd: {:x}", value);
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

    Ok(ExitStatus::default())
}