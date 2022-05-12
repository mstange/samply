use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use crate::perf_event::{CpuMode, Event, RawEvent};
use crate::perf_event_raw::{
    PerfEventAttr, ATTR_FLAG_BIT_SAMPLE_ID_ALL, PERF_RECORD_MISC_BUILD_ID_SIZE,
};
use crate::raw_data::RawData;
use crate::reader::Reader;
use crate::unaligned::{Endianness, U16, U32, U64};
use zerocopy::FromBytes;

pub struct PerfFile {
    event_data_offset_and_size: (u64, u64),

    /// The list of global opcodes.
    perf_event_attrs: Vec<PerfEventAttr>,

    feature_sections: Vec<(FlagFeature, Vec<u8>)>,

    endian: Endianness,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DsoBuildId {
    pub path: Vec<u8>,
    pub build_id: Vec<u8>,
}

impl PerfFile {
    pub fn parse<C>(mut cursor: C) -> Result<Self, Error>
    where
        C: Read + Seek,
    {
        let start_offset = cursor.stream_position()?;
        let mut header_bytes = [0; std::mem::size_of::<PerfHeader>()];
        cursor.read_exact(&mut header_bytes)?;
        let header = PerfHeader::parse(&header_bytes)?;
        if &header.magic != b"PERFILE2" && &header.magic != b"2ELIFREP" {
            return Err(Error::UnrecognizedMagicValue(header.magic));
        }
        let endian = if header.magic[0] == b'P' {
            Endianness::LittleEndian
        } else {
            Endianness::BigEndian
        };

        // Read the section information for each flag, starting just after the data section.
        let mut flag = 0u32;
        let feature_pos = header.data.offset.get(endian) + header.data.size.get(endian);
        cursor.seek(SeekFrom::Start(start_offset + feature_pos))?;
        let mut feature_sections_info = Vec::new();
        for flags_chunk in header.flags {
            let flags_chunk = flags_chunk.get(endian);
            for bit_index in 0..8 {
                let flag_is_set = (flags_chunk & (1 << bit_index)) != 0;
                if flag_is_set {
                    let mut section_bytes = [0; std::mem::size_of::<PerfFileSection>()];
                    cursor.read_exact(&mut section_bytes)?;
                    let section = section_bytes
                        .read_at::<PerfFileSection>(0)
                        .ok_or(ReadError::PerFlagSection)?;
                    if let Some(feature) = FlagFeature::from_int(flag) {
                        feature_sections_info.push((feature, *section));
                    } else {
                        eprintln!("Unrecognized flag feature {}", flag);
                    }
                }
                flag += 1;
            }
        }

        let mut feature_sections = Vec::new();
        for (feature, section) in feature_sections_info {
            let offset = section.offset.get(endian);
            let size =
                usize::try_from(section.size.get(endian)).map_err(|_| Error::SectionSizeTooBig)?;
            let mut data = vec![0; size];
            cursor.seek(SeekFrom::Start(start_offset + offset))?;
            cursor.read_exact(&mut data)?;
            feature_sections.push((feature, data));
        }

        let attrs_offset = header.attrs.offset.get(endian);
        let attrs_size = header.attrs.size.get(endian);
        let attrs_size = usize::try_from(attrs_size).map_err(|_| Error::SectionSizeTooBig)?;
        cursor.seek(SeekFrom::Start(start_offset + attrs_offset))?;
        let mut attrs_section_data = vec![0; attrs_size];
        cursor.read_exact(&mut attrs_section_data)?;
        let mut perf_event_attrs = Vec::new();
        let attr_size = header.attr_size.get(endian);
        let mut offset = 0;
        while offset < attrs_size as u64 {
            let attr = attrs_section_data
                .read_at::<PerfEventAttr>(offset)
                .ok_or(ReadError::PerfEventAttr)?;
            perf_event_attrs.push(*attr);
            offset += attr_size;
        }
        // eprintln!("Got {} perf_event_attrs.", perf_event_attrs.len());
        // for perf_event_attr in &perf_event_attrs {
        //     eprintln!("flags: {:b}", perf_event_attr.flags.get(endian));
        //     if perf_event_attr.flags.get(endian) & ATTR_FLAG_BIT_ENABLE_ON_EXEC != 0 {
        //         eprintln!("ATTR_FLAG_BIT_ENABLE_ON_EXEC is set");
        //     }
        // }

        Ok(Self {
            event_data_offset_and_size: (
                header.data.offset.get(endian),
                header.data.size.get(endian),
            ),
            perf_event_attrs,
            feature_sections,
            endian,
        })
    }

    pub fn endian(&self) -> Endianness {
        self.endian
    }

    pub fn has_feature(&self, feature: FlagFeature) -> bool {
        self.feature_sections.iter().any(|(f, _)| *f == feature)
    }

    pub fn feature_section(&self, feature: FlagFeature) -> Option<&[u8]> {
        self.feature_sections
            .iter()
            .find_map(|(f, d)| if *f == feature { Some(&d[..]) } else { None })
    }

    /// Returns a map of build ID entries. `perf record` creates these records for any DSOs
    /// which it thinks have been "hit" in the profile. They supplement Mmap events
    /// the perf event stream; those usually don't come with build IDs.
    ///
    /// This method returns a HashMap so that you can easily look up the right build ID from
    /// the DsoKey in an Mmap event. For some DSOs, the path in the raw Mmap event can be
    /// different from the path in the build ID record; for example, the Mmap event for the
    /// kernel ("vmlinux") image could have the path "[kernel.kallsyms]_text", whereas the
    /// corresponding build ID record might have the path "[kernel.kallsyms]" (without the
    /// trailing "_text"), or it could even have the full absolute path to a vmlinux file.
    /// The DsoKey canonicalizes those differences away.
    ///
    /// Having the build ID for a DSO allows you to do the following:
    ///
    ///  - If the DSO file has changed in the time since the perf.data file was captured,
    ///    you can detect this change because the new file will have a different build ID.
    ///  - If debug symbols are installed for the DSO, you can sometimes find the debug symbol
    ///    file using the build ID. For example, you might find it at
    ///    /usr/lib/debug/.build-id/b8/037b6260865346802321dd2256b8ad1d857e63.debug
    ///  - If the original DSO file is gone, or you're trying to read the perf.data file on
    ///    an entirely different machine, you can sometimes retrieve the original DSO file just
    ///    from its build ID, for example from a debuginfod server.
    ///  - This also works for DSOs which are not present on the file system at all;
    ///    specifically, the vDSO file is a bit of a pain to obtain. With the build ID you can
    ///    instead obtain it from, say,
    ///    https://debuginfod.elfutils.org/buildid/0d82ee4bd7f9609c367095ba0bedf155b71cb058/executable
    ///
    /// This method is a bit lossy. We discard the pid, because it seems to be always -1 in
    /// the files I've tested. We also discard any entries for which we fail to create a `DsoKey`.
    pub fn build_ids(&self) -> Result<HashMap<DsoKey, DsoBuildId>, Error> {
        let section_data = match self.feature_section(FlagFeature::BuildId) {
            Some(section) => section,
            None => return Ok(HashMap::new()),
        };
        let mut offset = 0;
        let mut build_ids = HashMap::new();
        while let Some(build_id_event) = section_data.read_at::<BuildIdEvent>(offset as u64) {
            let file_name_start = offset + std::mem::size_of::<BuildIdEvent>();
            let record_end = offset + build_id_event.header.size.get(self.endian) as usize;
            offset = record_end;
            let file_name_bytes = &section_data[file_name_start..record_end];
            let file_name_len = memchr::memchr(0, file_name_bytes).unwrap_or(record_end);
            let path = &file_name_bytes[..file_name_len];
            let misc = build_id_event.header.misc.get(self.endian);
            let dso_key = match DsoKey::detect(path, CpuMode::from_misc(misc)) {
                Some(dso_key) => dso_key,
                None => continue,
            };
            let build_id_len = if misc & PERF_RECORD_MISC_BUILD_ID_SIZE != 0 {
                build_id_event.build_id[20].min(20)
            } else {
                detect_build_id_len(&build_id_event.build_id)
            };
            let build_id = build_id_event.build_id[..build_id_len as usize].to_owned();
            let path = path.to_owned();
            build_ids.insert(dso_key, DsoBuildId { path, build_id });
        }
        Ok(build_ids)
    }

    pub fn events<'a, 'b: 'a>(&'a self, data: &'b [u8]) -> EventIter<'a> {
        EventIter {
            data,
            event_data_offset_and_size: self.event_data_offset_and_size,
            attr: &self.perf_event_attrs[0],
            endian: self.endian,
            offset: 0,
        }
    }

    /// Only call this for features whose section is just a perf_header_string.
    fn feature_string(&self, feature: FlagFeature) -> Result<Option<&str>, Error> {
        let section_data = match self.feature_section(feature) {
            Some(section) => section,
            None => return Ok(None),
        };
        let s = self.read_string(section_data)?;
        Ok(Some(s))
    }

    pub fn hostname(&self) -> Result<Option<&str>, Error> {
        self.feature_string(FlagFeature::Hostname)
    }

    pub fn os_release(&self) -> Result<Option<&str>, Error> {
        self.feature_string(FlagFeature::OsRelease)
    }

    pub fn perf_version(&self) -> Result<Option<&str>, Error> {
        self.feature_string(FlagFeature::Version)
    }

    pub fn arch(&self) -> Result<Option<&str>, Error> {
        self.feature_string(FlagFeature::Arch)
    }

    pub fn nr_cpus(&self) -> Result<Option<&NrCpus>, Error> {
        let section_data = match self.feature_section(FlagFeature::NrCpus) {
            Some(section) => section,
            None => return Ok(None),
        };
        if section_data.len() < std::mem::size_of::<NrCpus>() {
            return Err(Error::NotEnoughSpaceForNrCpus);
        }
        let nr_cpus = section_data.read_at::<NrCpus>(0).ok_or(ReadError::NrCpus)?;
        Ok(Some(nr_cpus))
    }

    pub fn is_stats(&self) -> bool {
        self.has_feature(FlagFeature::Stat)
    }

    fn read_string<'a>(&self, s: &'a [u8]) -> Result<&'a str, Error> {
        if s.len() < 4 {
            return Err(Error::NotEnoughSpaceForStringLen);
        }
        let (len_bytes, rest) = s.split_at(4);
        let len = U32([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        let len = len.get(self.endian);
        let len = usize::try_from(len).map_err(|_| Error::StringLengthBiggerThanUsize)?;
        let s = &rest.get(..len as usize).ok_or(Error::StringLengthTooLong)?;
        let actual_len = memchr::memchr(0, s).unwrap_or(s.len());
        let s = std::str::from_utf8(&s[..actual_len]).map_err(|_| Error::StringUtf8)?;
        Ok(s)
    }
}

pub struct EventIter<'a> {
    data: &'a [u8],
    event_data_offset_and_size: (u64, u64),
    attr: &'a PerfEventAttr,
    endian: Endianness,
    offset: u64,
}

impl<'a> EventIter<'a> {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Event>, Error> {
        if self.offset >= self.event_data_offset_and_size.1 {
            return Ok(None);
        }
        let header = self
            .data
            .read_at::<PerfEventHeader>(self.event_data_offset_and_size.0 + self.offset)
            .ok_or(ReadError::PerfEventHeader)?;
        let kind = header.type_.get(self.endian);
        let misc = header.misc.get(self.endian);
        let size = header.size.get(self.endian);
        let event_data = self
            .data
            .read_slice_at::<u8>(
                self.event_data_offset_and_size.0
                    + self.offset
                    + std::mem::size_of::<PerfEventHeader>() as u64,
                size as usize - std::mem::size_of::<PerfEventHeader>() as usize,
            )
            .ok_or(ReadError::PerfEventData)?;
        self.offset += size as u64;
        let raw_data = RawData::from(event_data);
        let raw_event = RawEvent {
            kind,
            misc,
            data: raw_data,
        };
        let sample_type = self.attr.sample_type.get(self.endian);
        let read_format = self.attr.read_format.get(self.endian);
        let sample_id_all = self.attr.flags.get(self.endian) & ATTR_FLAG_BIT_SAMPLE_ID_ALL != 0;
        let sample_regs_user = self.attr.sample_regs_user.get(self.endian);
        let regs_count = sample_regs_user.count_ones() as usize;
        let event = if self.endian == Endianness::LittleEndian {
            raw_event.parse::<byteorder::LittleEndian>(
                sample_type,
                read_format,
                regs_count,
                sample_regs_user,
                sample_id_all,
            )
        } else {
            raw_event.parse::<byteorder::BigEndian>(
                sample_type,
                read_format,
                regs_count,
                sample_regs_user,
                sample_id_all,
            )
        };

        Ok(Some(event))
    }
}

/// Old versions of perf did not write down the length of the build ID.
/// Detect the true length by removing 4-byte chunks of zeros from the end.
fn detect_build_id_len(build_id_bytes: &[u8]) -> u8 {
    let mut len = build_id_bytes.len();
    const CHUNK_SIZE: usize = 4;
    for chunk in build_id_bytes.chunks(CHUNK_SIZE).rev() {
        if chunk.iter().any(|b| *b != 0) {
            break;
        }
        len -= chunk.len();
    }
    len as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagFeature {
    TracingData,
    BuildId,
    Hostname,
    OsRelease,
    Version,
    Arch,
    NrCpus,
    CpuDesc,
    CpuId,
    TotalMem,
    Cmdline,
    EventDesc,
    CpuTopology,
    NumaTopology,
    BranchStack,
    PmuMappings,
    GroupDesc,
    Auxtrace,
    Stat,
    Cache,
    SampleTime,
    SampleTopology,
    ClockId,
    DirFormat,
    CpuPmuCaps,
    ClockData,
    HybridTopology,
    HybridCpuPmuCaps,
}

impl FlagFeature {
    pub fn from_int(i: u32) -> Option<Self> {
        let feature = match i {
            HEADER_TRACING_DATA => Self::TracingData,
            HEADER_BUILD_ID => Self::BuildId,
            HEADER_HOSTNAME => Self::Hostname,
            HEADER_OSRELEASE => Self::OsRelease,
            HEADER_VERSION => Self::Version,
            HEADER_ARCH => Self::Arch,
            HEADER_NRCPUS => Self::NrCpus,
            HEADER_CPUDESC => Self::CpuDesc,
            HEADER_CPUID => Self::CpuId,
            HEADER_TOTAL_MEM => Self::TotalMem,
            HEADER_CMDLINE => Self::Cmdline,
            HEADER_EVENT_DESC => Self::EventDesc,
            HEADER_CPU_TOPOLOGY => Self::CpuTopology,
            HEADER_NUMA_TOPOLOGY => Self::NumaTopology,
            HEADER_BRANCH_STACK => Self::BranchStack,
            HEADER_PMU_MAPPINGS => Self::PmuMappings,
            HEADER_GROUP_DESC => Self::GroupDesc,
            HEADER_AUXTRACE => Self::Auxtrace,
            HEADER_STAT => Self::Stat,
            HEADER_CACHE => Self::Cache,
            HEADER_SAMPLE_TIME => Self::SampleTime,
            HEADER_SAMPLE_TOPOLOGY => Self::SampleTopology,
            HEADER_CLOCKID => Self::ClockId,
            HEADER_DIR_FORMAT => Self::DirFormat,
            HEADER_CPU_PMU_CAPS => Self::CpuPmuCaps,
            HEADER_CLOCK_DATA => Self::ClockData,
            HEADER_HYBRID_TOPOLOGY => Self::HybridTopology,
            HEADER_HYBRID_CPU_PMU_CAPS => Self::HybridCpuPmuCaps,
            _ => return None,
        };
        Some(feature)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DsoKey {
    Kernel,
    GuestKernel,
    Vdso32,
    VdsoX32,
    Vdso64,
    Vsyscall,
    KernelModule(String),
    User(String, Vec<u8>),
}

impl DsoKey {
    pub fn detect(path: &[u8], cpu_mode: CpuMode) -> Option<Self> {
        if path == b"//anon" || path == b"[stack]" || path == b"[heap]" || path == b"[vvar]" {
            return None;
        }

        if path.starts_with(b"[kernel.kallsyms]") {
            let dso_key = if cpu_mode == CpuMode::GuestKernel {
                DsoKey::GuestKernel
            } else {
                DsoKey::Kernel
            };
            return Some(dso_key);
        }
        if path.starts_with(b"[guest.kernel.kallsyms") {
            return Some(DsoKey::GuestKernel);
        }
        if path == b"[vdso32]" {
            return Some(DsoKey::Vdso32);
        }
        if path == b"[vdsox32]" {
            return Some(DsoKey::VdsoX32);
        }
        if path == b"[vdso]" {
            // TODO: I think this could also be Vdso32 when recording on a 32 bit machine.
            return Some(DsoKey::Vdso64);
        }
        if path == b"[vsyscall]" {
            return Some(DsoKey::Vsyscall);
        }
        if (cpu_mode == CpuMode::Kernel || cpu_mode == CpuMode::GuestKernel)
            && path.starts_with(b"[")
        {
            return Some(DsoKey::KernelModule(String::from_utf8_lossy(path).into()));
        }

        let filename = if let Some(final_slash_pos) = path.iter().rposition(|b| *b == b'/') {
            &path[final_slash_pos + 1..]
        } else {
            path
        };

        let dso_key = match (cpu_mode, filename.strip_suffix(b".ko")) {
            (CpuMode::Kernel | CpuMode::GuestKernel, Some(kmod_name)) => {
                // "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-seq-device.ko" -> "[snd-seq-device]"
                let kmod_name = String::from_utf8_lossy(kmod_name);
                DsoKey::KernelModule(format!("[{}]", kmod_name))
            }
            (CpuMode::Kernel, _) => DsoKey::Kernel,
            (CpuMode::GuestKernel, _) => DsoKey::GuestKernel,
            (CpuMode::User | CpuMode::GuestUser, _) => {
                DsoKey::User(String::from_utf8_lossy(filename).into(), path.to_owned())
            }
            _ => return None,
        };
        Some(dso_key)
    }

    pub fn name(&self) -> &str {
        match self {
            DsoKey::Kernel => "[kernel.kallsyms]",
            DsoKey::GuestKernel => "[guest.kernel.kallsyms]",
            DsoKey::Vdso32 => "[vdso32]",
            DsoKey::VdsoX32 => "[vdsox32]",
            DsoKey::Vdso64 => "[vdso]",
            DsoKey::Vsyscall => "[vsyscall]",
            DsoKey::KernelModule(name) => name,
            DsoKey::User(name, _) => name,
        }
    }
}

/// `perf_header`
///
/// The magic number identifies the perf file and the version. Current perf versions
/// use PERFILE2. Old perf versions generated a version 1 format (PERFFILE). Version 1
/// is not described here. The magic number also identifies the endian. When the
/// magic value is 64bit byte swapped compared the file is in non-native
/// endian.

#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct PerfHeader {
    /// b"PERFILE2" for little-endian, b"2ELIFREP" for big-endian
    pub magic: [u8; 8],
    /// size of the header
    pub size: U64,
    /// size of an attribute in attrs
    pub attr_size: U64,
    pub attrs: PerfFileSection,
    pub data: PerfFileSection,
    /// Ignored
    pub event_types: PerfFileSection,
    /// Room for 4 * 64 = 256 header flag bits
    pub flags: [U64; 4],
}

pub const HEADER_TRACING_DATA: u32 = 1;
pub const HEADER_BUILD_ID: u32 = 2;
pub const HEADER_HOSTNAME: u32 = 3;
pub const HEADER_OSRELEASE: u32 = 4;
pub const HEADER_VERSION: u32 = 5;
pub const HEADER_ARCH: u32 = 6;
pub const HEADER_NRCPUS: u32 = 7;
pub const HEADER_CPUDESC: u32 = 8;
pub const HEADER_CPUID: u32 = 9;
pub const HEADER_TOTAL_MEM: u32 = 10;
pub const HEADER_CMDLINE: u32 = 11;
pub const HEADER_EVENT_DESC: u32 = 12;
pub const HEADER_CPU_TOPOLOGY: u32 = 13;
pub const HEADER_NUMA_TOPOLOGY: u32 = 14;
pub const HEADER_BRANCH_STACK: u32 = 15;
pub const HEADER_PMU_MAPPINGS: u32 = 16;
pub const HEADER_GROUP_DESC: u32 = 17;
pub const HEADER_AUXTRACE: u32 = 18;
pub const HEADER_STAT: u32 = 19;
pub const HEADER_CACHE: u32 = 20;
pub const HEADER_SAMPLE_TIME: u32 = 21;
pub const HEADER_SAMPLE_TOPOLOGY: u32 = 22;
pub const HEADER_CLOCKID: u32 = 23;
pub const HEADER_DIR_FORMAT: u32 = 24;
pub const HEADER_CPU_PMU_CAPS: u32 = 28;
pub const HEADER_CLOCK_DATA: u32 = 29;
pub const HEADER_HYBRID_TOPOLOGY: u32 = 30;
pub const HEADER_HYBRID_CPU_PMU_CAPS: u32 = 31;

/// `perf_file_section`
///
/// A PerfFileSection contains a pointer to another section of the perf file.
/// The header contains three such pointers: for attributes, data and event types.
#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct PerfFileSection {
    /// offset from start of file
    pub offset: U64,
    /// size of the section
    pub size: U64,
}

// type Result<T> = std::result::Result<T, ReadError>;

impl PerfHeader {
    pub fn parse(data: &[u8]) -> Result<&Self, ReadError> {
        data.read_at::<PerfHeader>(0).ok_or(ReadError::PerfHeader)
    }
}

/// `perf_event_header`
#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct PerfEventHeader {
    pub type_: U32,
    pub misc: U16,
    pub size: U16,
}

/// `build_id_event`
#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct BuildIdEvent {
    /// If PERF_RECORD_MISC_KERNEL is set in header.misc, then this
    /// is the build id for the vmlinux image or a kmod.
    pub header: PerfEventHeader,
    pub pid: U32, // probably rather I32
    /// If PERF_RECORD_MISC_BUILD_ID_SIZE is set in header.misc, then build_id[20]
    /// is the length of the build id (<= 20), and build_id[21..24] are unused.
    /// Otherwise, the length of the build ID is unknown and has to be detected by
    /// removing trailing 4-byte groups of zero bytes. (Usually there will be
    /// exactly one such group, because build IDs are usually 20 bytes long.)
    pub build_id: [u8; 24],
    // Followed by filename for the remaining bytes. The total size of the record
    // is given by self.header.size.
}

/// `nr_cpus`
#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct NrCpus {
    /// CPUs not yet onlined
    pub nr_cpus_available: U32,
    pub nr_cpus_online: U32,
}

/// The error type used in this crate.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The data slice was not big enough to read the struct, or we
    /// were trying to follow an invalid offset to somewhere outside
    /// of the data bounds.
    #[error("Read error: {0}")]
    Read(#[from] ReadError),

    #[error("I/O error: {0}")]
    IoError(#[from] io::Error),

    #[error("Did not recognize magic value {0:?}")]
    UnrecognizedMagicValue([u8; 8]),

    #[error("Section size did not fit into usize")]
    SectionSizeTooBig,

    #[error("The section wasn't big enough to contain the u32 string length")]
    NotEnoughSpaceForStringLen,

    #[error("The section wasn't big enough to contain the NrCpus struct")]
    NotEnoughSpaceForNrCpus,

    #[error("The indicated string length wouldn't fit in the indicated section size")]
    StringLengthTooLong,

    #[error("The indicated string length wouldn't fit into usize")]
    StringLengthBiggerThanUsize,

    #[error("The string was not valid utf-8")]
    StringUtf8,
}

/// This error indicates that the data slice was not large enough to
/// read the respective item.
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    #[error("Could not read PerfHeader")]
    PerfHeader,

    #[error("Could not read PerFlagSection")]
    PerFlagSection,

    #[error("Could not read BuildIdSection")]
    BuildIdSection,

    #[error("Could not read StringLen")]
    StringLen,

    #[error("Could not read String")]
    String,

    #[error("Could not read NrCpus")]
    NrCpus,

    #[error("Could not read AttrsSection")]
    AttrsSection,

    #[error("Could not read PerfEventAttr")]
    PerfEventAttr,

    #[error("Could not read PerfEventHeader")]
    PerfEventHeader,

    #[error("Could not read PerfEvent data")]
    PerfEventData,
}
