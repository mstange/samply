use crate::perf_event::{Event, RawEvent};
use crate::perf_event_raw::{
    PerfEventAttr, ATTR_FLAG_BIT_ENABLE_ON_EXEC, ATTR_FLAG_BIT_SAMPLE_ID_ALL,
};
use crate::raw_data::RawData;
use crate::reader::Reader;
use crate::unaligned::{Endianness, U16, U32, U64};
use zerocopy::FromBytes;

pub struct PerfFile<'a> {
    /// The full file data.
    data: &'a [u8],

    event_data_offset_and_size: (u64, u64),

    /// The list of global opcodes.
    perf_event_attrs: Vec<&'a PerfEventAttr>,

    feature_sections: Vec<(FlagFeature, &'a PerfFileSection)>,

    endian: Endianness,
}

impl<'a> PerfFile<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let header = PerfHeader::parse(data)?;
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
        let mut pos = header.data.offset.get(endian) + header.data.size.get(endian);
        let mut feature_sections = Vec::new();
        for flags_chunk in header.flags {
            let flags_chunk = flags_chunk.get(endian);
            for bit_index in 0..8 {
                let flag_is_set = (flags_chunk & (1 << bit_index)) != 0;
                if flag_is_set {
                    let section = data
                        .read_at::<PerfFileSection>(pos)
                        .ok_or(ReadError::PerFlagSection)?;
                    pos += std::mem::size_of::<PerfFileSection>() as u64;
                    if let Some(feature) = FlagFeature::from_int(flag) {
                        feature_sections.push((feature, section));
                    } else {
                        eprintln!("Unrecognized flag feature {}", flag);
                    }
                }
                flag += 1;
            }
        }

        let attrs_offset = header.attrs.offset.get(endian);
        let attrs_size = header.attrs.size.get(endian);
        let attrs_size = usize::try_from(attrs_size).map_err(|_| Error::SectionSizeTooBig)?;
        let attrs_section_data = data
            .read_slice_at::<u8>(attrs_offset, attrs_size)
            .ok_or(ReadError::AttrsSection)?;
        let mut perf_event_attrs = Vec::new();
        let attr_size = header.attr_size.get(endian);
        let mut offset = 0;
        while offset < attrs_size as u64 {
            let attr = attrs_section_data
                .read_at::<PerfEventAttr>(offset)
                .ok_or(ReadError::PerfEventAttr)?;
            perf_event_attrs.push(attr);
            offset += attr_size;
        }
        eprintln!("Got {} perf_event_attrs.", perf_event_attrs.len());
        for perf_event_attr in &perf_event_attrs {
            eprintln!("flags: {:b}", perf_event_attr.flags.get(endian));
            if perf_event_attr.flags.get(endian) & ATTR_FLAG_BIT_ENABLE_ON_EXEC != 0 {
                eprintln!("ATTR_FLAG_BIT_ENABLE_ON_EXEC is set");
            }
        }

        Ok(Self {
            data,
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

    pub fn feature_section(&self, feature: FlagFeature) -> Option<(u64, u64)> {
        match self.feature_sections.iter().find(|(f, _)| *f == feature) {
            Some((_, section)) => {
                let offset = section.offset.get(self.endian);
                let size = section.size.get(self.endian);
                Some((offset, size))
            }
            None => None,
        }
    }

    #[allow(clippy::complexity)]
    pub fn build_ids(&self) -> Result<Option<Vec<(&'a BuildIdEvent, &'a [u8])>>, Error> {
        let (offset, size) = match self.feature_section(FlagFeature::BuildId) {
            Some(section) => section,
            None => return Ok(None),
        };
        let size = usize::try_from(size).map_err(|_| Error::SectionSizeTooBig)?;
        let section_data = self
            .data
            .read_slice_at::<u8>(offset, size)
            .ok_or(ReadError::BuildIdSection)?;
        let mut offset = 0;
        let mut build_ids = Vec::new();
        while let Some(build_id_event) = section_data.read_at::<BuildIdEvent>(offset as u64) {
            let file_name_start = offset + std::mem::size_of::<BuildIdEvent>();
            let record_end = offset + build_id_event.header.size.get(self.endian) as usize;
            offset = record_end;
            let file_name_bytes = &section_data[file_name_start..record_end];
            let file_name_len = memchr::memchr(0, file_name_bytes).unwrap_or(record_end);
            let file_name = &file_name_bytes[..file_name_len];
            build_ids.push((build_id_event, file_name));
        }
        Ok(Some(build_ids))
    }

    pub fn events(&self) -> EventIter<'a> {
        EventIter {
            data: self.data,
            event_data_offset_and_size: self.event_data_offset_and_size,
            attr: self.perf_event_attrs[0],
            endian: self.endian,
            offset: 0,
        }
    }

    /// Only call this for features whose section is just a perf_header_string.
    fn feature_string(&self, feature: FlagFeature) -> Result<Option<&'a str>, Error> {
        let (offset, size) = match self.feature_section(feature) {
            Some(section) => section,
            None => return Ok(None),
        };
        let s = self.read_string(offset, size)?;
        Ok(Some(s))
    }

    pub fn hostname(&self) -> Result<Option<&'a str>, Error> {
        self.feature_string(FlagFeature::Hostname)
    }

    pub fn os_release(&self) -> Result<Option<&'a str>, Error> {
        self.feature_string(FlagFeature::OsRelease)
    }

    pub fn perf_version(&self) -> Result<Option<&'a str>, Error> {
        self.feature_string(FlagFeature::Version)
    }

    pub fn arch(&self) -> Result<Option<&'a str>, Error> {
        self.feature_string(FlagFeature::Arch)
    }

    pub fn nr_cpus(&self) -> Result<Option<&'a NrCpus>, Error> {
        let (offset, size) = match self.feature_section(FlagFeature::NrCpus) {
            Some(section) => section,
            None => return Ok(None),
        };
        if size < std::mem::size_of::<NrCpus>() as u64 {
            return Err(Error::NotEnoughSpaceForNrCpus);
        }
        let nr_cpus = self
            .data
            .read_at::<NrCpus>(offset)
            .ok_or(ReadError::NrCpus)?;
        Ok(Some(nr_cpus))
    }

    pub fn is_stats(&self) -> bool {
        self.has_feature(FlagFeature::Stat)
    }

    fn read_string(&self, offset: u64, size: u64) -> Result<&'a str, Error> {
        if size < 4 {
            return Err(Error::NotEnoughSpaceForStringLen);
        }
        let len = self
            .data
            .read_at::<U32>(offset)
            .ok_or(ReadError::StringLen)?;
        let len = u64::from(len.get(self.endian));
        if 4 + len > size {
            return Err(Error::StringLengthTooLong);
        }
        let string_start = offset + 4;
        let len = usize::try_from(len).map_err(|_| Error::StringLengthBiggerThanUsize)?;
        let s = self
            .data
            .read_slice_at::<u8>(string_start, len)
            .ok_or(ReadError::String)?;
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
    pub header: PerfEventHeader,
    pub pid: U32, // probably rather I32
    pub build_id: [u8; 24],
    // Followed by filename for the remaining bytes. The total size of the record is given by self.header.size.
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
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The data slice was not big enough to read the struct, or we
    /// were trying to follow an invalid offset to somewhere outside
    /// of the data bounds.
    #[error("Read error: {0}")]
    Read(#[from] ReadError),

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
