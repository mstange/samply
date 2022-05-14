use crate::perf_event_consts::*;
use crate::raw_data::{RawData, RawRegs};
use crate::utils::HexValue;
use bitflags::bitflags;
use byteorder::{ByteOrder, ReadBytesExt};
use std::io::Read;
use std::{fmt, io};

bitflags! {
    pub struct SampleFormat: u64 {
        const IP = PERF_SAMPLE_IP;
        const TID = PERF_SAMPLE_TID;
        const TIME = PERF_SAMPLE_TIME;
        const ADDR = PERF_SAMPLE_ADDR;
        const READ = PERF_SAMPLE_READ;
        const CALLCHAIN = PERF_SAMPLE_CALLCHAIN;
        const ID = PERF_SAMPLE_ID;
        const CPU = PERF_SAMPLE_CPU;
        const PERIOD = PERF_SAMPLE_PERIOD;
        const STREAM_ID = PERF_SAMPLE_STREAM_ID;
        const RAW = PERF_SAMPLE_RAW;
        const BRANCH_STACK = PERF_SAMPLE_BRANCH_STACK;
        const REGS_USER = PERF_SAMPLE_REGS_USER;
        const STACK_USER = PERF_SAMPLE_STACK_USER;
        const WEIGHT = PERF_SAMPLE_WEIGHT;
        const DATA_SRC = PERF_SAMPLE_DATA_SRC;
        const IDENTIFIER = PERF_SAMPLE_IDENTIFIER;
        const TRANSACTION = PERF_SAMPLE_TRANSACTION;
        const REGS_INTR = PERF_SAMPLE_REGS_INTR;
        const PHYS_ADDR = PERF_SAMPLE_PHYS_ADDR;
        const AUX = PERF_SAMPLE_AUX;
        const CGROUP = PERF_SAMPLE_CGROUP;
        const DATA_PAGE_SIZE = PERF_SAMPLE_DATA_PAGE_SIZE;
        const CODE_PAGE_SIZE = PERF_SAMPLE_CODE_PAGE_SIZE;
        const WEIGHT_STRUCT = PERF_SAMPLE_WEIGHT_STRUCT;
    }

    pub struct BranchSampleFormat: u64 {
        /// user branches
        const USER = PERF_SAMPLE_BRANCH_USER;
        /// kernel branches
        const KERNEL = PERF_SAMPLE_BRANCH_KERNEL;
        /// hypervisor branches
        const HV = PERF_SAMPLE_BRANCH_HV;
        /// any branch types
        const ANY = PERF_SAMPLE_BRANCH_ANY;
        /// any call branch
        const ANY_CALL = PERF_SAMPLE_BRANCH_ANY_CALL;
        /// any return branch
        const ANY_RETURN = PERF_SAMPLE_BRANCH_ANY_RETURN;
        /// indirect calls
        const IND_CALL = PERF_SAMPLE_BRANCH_IND_CALL;
        /// transaction aborts
        const ABORT_TX = PERF_SAMPLE_BRANCH_ABORT_TX;
        /// in transaction
        const IN_TX = PERF_SAMPLE_BRANCH_IN_TX;
        /// not in transaction
        const NO_TX = PERF_SAMPLE_BRANCH_NO_TX;
        /// conditional branches
        const COND = PERF_SAMPLE_BRANCH_COND;
        /// call/ret stack
        const CALL_STACK = PERF_SAMPLE_BRANCH_CALL_STACK;
        /// indirect jumps
        const IND_JUMP = PERF_SAMPLE_BRANCH_IND_JUMP;
        /// direct call
        const CALL = PERF_SAMPLE_BRANCH_CALL;
        /// no flags
        const NO_FLAGS = PERF_SAMPLE_BRANCH_NO_FLAGS;
        /// no cycles
        const NO_CYCLES = PERF_SAMPLE_BRANCH_NO_CYCLES;
        /// save branch type
        const TYPE_SAVE = PERF_SAMPLE_BRANCH_TYPE_SAVE;
        /// save low level index of raw branch records
        const HW_INDEX = PERF_SAMPLE_BRANCH_HW_INDEX;
    }

    pub struct AttrFlags: u64 {
        /// off by default
        const DISABLED = ATTR_FLAG_BIT_DISABLED;
        /// children inherit it
        const INHERIT = ATTR_FLAG_BIT_INHERIT;
        /// must always be on PMU
        const PINNED = ATTR_FLAG_BIT_PINNED;
        /// only group on PMU
        const EXCLUSIVE = ATTR_FLAG_BIT_EXCLUSIVE;
        /// don't count user
        const EXCLUDE_USER = ATTR_FLAG_BIT_EXCLUDE_USER;
        /// don't count kernel
        const EXCLUDE_KERNEL = ATTR_FLAG_BIT_EXCLUDE_KERNEL;
        /// don't count hypervisor
        const EXCLUDE_HV = ATTR_FLAG_BIT_EXCLUDE_HV;
        /// don't count when idle
        const EXCLUDE_IDLE = ATTR_FLAG_BIT_EXCLUDE_IDLE;
        /// include mmap data
        const MMAP = ATTR_FLAG_BIT_MMAP;
        /// include comm data
        const COMM = ATTR_FLAG_BIT_COMM;
        /// use freq, not period
        const FREQ = ATTR_FLAG_BIT_FREQ;
        /// per task counts
        const INHERIT_STAT = ATTR_FLAG_BIT_INHERIT_STAT;
        /// next exec enables
        const ENABLE_ON_EXEC = ATTR_FLAG_BIT_ENABLE_ON_EXEC;
        /// trace fork/exit
        const TASK = ATTR_FLAG_BIT_TASK;
        /// wakeup_watermark
        const WATERMARK = ATTR_FLAG_BIT_WATERMARK;
        /// one of the two PRECISE_IP bitmask bits
        const PRECISE_IP_BIT_15 = 1 << 15;
        /// one of the two PRECISE_IP bitmask bits
        const PRECISE_IP_BIT_16 = 1 << 16;
        /// the full PRECISE_IP bitmask
        const PRECISE_IP_BITMASK = ATTR_FLAG_BITMASK_PRECISE_IP;
        /// non-exec mmap data
        const MMAP_DATA = ATTR_FLAG_BIT_MMAP_DATA;
        /// sample_type all events
        const SAMPLE_ID_ALL = ATTR_FLAG_BIT_SAMPLE_ID_ALL;
        /// don't count in host
        const EXCLUDE_HOST = ATTR_FLAG_BIT_EXCLUDE_HOST;
        /// don't count in guest
        const EXCLUDE_GUEST = ATTR_FLAG_BIT_EXCLUDE_GUEST;
        /// exclude kernel callchains
        const EXCLUDE_CALLCHAIN_KERNEL = ATTR_FLAG_BIT_EXCLUDE_CALLCHAIN_KERNEL;
        /// exclude user callchains
        const EXCLUDE_CALLCHAIN_USER = ATTR_FLAG_BIT_EXCLUDE_CALLCHAIN_USER;
        /// include mmap with inode data
        const MMAP2 = ATTR_FLAG_BIT_MMAP2;
        /// flag comm events that are due to exec
        const COMM_EXEC = ATTR_FLAG_BIT_COMM_EXEC;
        /// use @clockid for time fields
        const USE_CLOCKID = ATTR_FLAG_BIT_USE_CLOCKID;
        /// context switch data
        const CONTEXT_SWITCH = ATTR_FLAG_BIT_CONTEXT_SWITCH;
        /// Write ring buffer from end to beginning
        const WRITE_BACKWARD = ATTR_FLAG_BIT_WRITE_BACKWARD;
        /// include namespaces data
        const NAMESPACES = ATTR_FLAG_BIT_NAMESPACES;
        /// include ksymbol events
        const KSYMBOL = ATTR_FLAG_BIT_KSYMBOL;
        /// include bpf events
        const BPF_EVENT = ATTR_FLAG_BIT_BPF_EVENT;
        /// generate AUX records instead of events
        const AUX_OUTPUT = ATTR_FLAG_BIT_AUX_OUTPUT;
        /// include cgroup events
        const CGROUP = ATTR_FLAG_BIT_CGROUP;
        /// include text poke events
        const TEXT_POKE = ATTR_FLAG_BIT_TEXT_POKE;
        /// use build id in mmap2 events
        const BUILD_ID = ATTR_FLAG_BIT_BUILD_ID;
        /// children only inherit if cloned with CLONE_THREAD
        const INHERIT_THREAD = ATTR_FLAG_BIT_INHERIT_THREAD;
        /// event is removed from task on exec
        const REMOVE_ON_EXEC = ATTR_FLAG_BIT_REMOVE_ON_EXEC;
        /// send synchronous SIGTRAP on event
        const SIGTRAP = ATTR_FLAG_BIT_SIGTRAP;
    }

    pub struct HwBreakpointType: u32 {
        const EMPTY = 0;
        const R = 1;
        const W = 2;
        const RW = Self::R.bits | Self::W.bits;
        const X = 4;
        const INVALID = Self::RW.bits | Self::X.bits;
    }

    /// The format of the data returned by read() on a perf event fd,
    /// as specified by attr.read_format:
    ///
    /// struct read_format {
    /// 	{ u64		value;
    /// 	  { u64		time_enabled; } && PERF_FORMAT_TOTAL_TIME_ENABLED
    /// 	  { u64		time_running; } && PERF_FORMAT_TOTAL_TIME_RUNNING
    /// 	  { u64		id;           } && PERF_FORMAT_ID
    /// 	} && !PERF_FORMAT_GROUP
    ///
    /// 	{ u64		nr;
    /// 	  { u64		time_enabled; } && PERF_FORMAT_TOTAL_TIME_ENABLED
    /// 	  { u64		time_running; } && PERF_FORMAT_TOTAL_TIME_RUNNING
    /// 	  { u64		value;
    /// 	    { u64	id;           } && PERF_FORMAT_ID
    /// 	  }		cntr[nr];
    /// 	} && PERF_FORMAT_GROUP
    /// };
    pub struct ReadFormat: u64 {
        const TOTAL_TIME_ENABLED = PERF_FORMAT_TOTAL_TIME_ENABLED;
        const TOTAL_TIME_RUNNING = PERF_FORMAT_TOTAL_TIME_RUNNING;
        const ID = PERF_FORMAT_ID;
        const GROUP = PERF_FORMAT_GROUP;
    }
}

/// Specifies how precise the instruction address should be.
/// With `perf record -e` you can set the precision by appending /p to the
/// event name, with varying numbers of `p`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IpSkidConstraint {
    /// 0 - SAMPLE_IP can have arbitrary skid
    ArbitrarySkid,
    /// 1 - SAMPLE_IP must have constant skid
    ConstantSkid,
    /// 2 - SAMPLE_IP requested to have 0 skid
    ZeroSkid,
    /// 3 - SAMPLE_IP must have 0 skid, or uses randomization to avoid
    /// sample shadowing effects.
    ZeroSkidOrRandomization,
}

impl AttrFlags {
    /// Extract the IpSkidConstraint from the bits.
    pub fn ip_skid_constraint(&self) -> IpSkidConstraint {
        match (self.bits & Self::PRECISE_IP_BITMASK.bits) >> 15 {
            0 => IpSkidConstraint::ArbitrarySkid,
            1 => IpSkidConstraint::ConstantSkid,
            2 => IpSkidConstraint::ZeroSkid,
            3 => IpSkidConstraint::ZeroSkidOrRandomization,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockId {
    Realtime,
    Monotonic,
    ProcessCputimeId,
    ThreadCputimeId,
    MonotonicRaw,
    RealtimeCoarse,
    MonotonicCoarse,
    Boottime,
    RealtimeAlarm,
    BoottimeAlarm,
}

impl ClockId {
    pub fn from_u32(clockid: u32) -> Option<Self> {
        Some(match clockid {
            0 => Self::Realtime,
            1 => Self::Monotonic,
            2 => Self::ProcessCputimeId,
            3 => Self::ThreadCputimeId,
            4 => Self::MonotonicRaw,
            5 => Self::RealtimeCoarse,
            6 => Self::MonotonicCoarse,
            7 => Self::Boottime,
            8 => Self::RealtimeAlarm,
            9 => Self::BoottimeAlarm,
            _ => return None,
        })
    }
}

/// `perf_event_header`
#[derive(Debug, Clone, Copy)]
pub struct PerfEventHeader {
    pub type_: u32,
    pub misc: u16,
    pub size: u16,
}

impl PerfEventHeader {
    pub const STRUCT_SIZE: usize = 4 + 2 + 2;

    pub fn parse<R: Read, T: ByteOrder>(reader: &mut R) -> Result<Self, std::io::Error> {
        let type_ = reader.read_u32::<T>()?;
        let misc = reader.read_u16::<T>()?;
        let size = reader.read_u16::<T>()?;
        Ok(Self { type_, misc, size })
    }
}

/// `perf_event_attr`
#[derive(Debug, Clone, Copy)]
pub struct PerfEventAttr {
    /// Major type: hardware/software/tracepoint/etc.
    pub type_: u32,
    /// Size of the attr structure, for fwd/bwd compat.
    pub size: u32,
    /// Type-specific configuration information.
    pub config: u64,

    /// If AttrFlags::FREQ is set in `flags`, this is the sample frequency,
    /// otherwise it is the sample period.
    ///
    /// ```c
    /// union {
    ///     /// Period of sampling
    ///     __u64 sample_period;
    ///     /// Frequency of sampling
    ///     __u64 sample_freq;
    /// };
    /// ```
    pub sampling_period_or_frequency: u64,

    /// Specifies values included in sample. (original name `sample_type`)
    pub sample_format: SampleFormat,

    /// Specifies the structure values returned by read() on a perf event fd,
    /// see [`ReadFormat`].
    pub read_format: ReadFormat,

    /// Bitset of flags.
    pub flags: AttrFlags,

    /// If AttrFlags::WATERMARK is set in `flags`, this is the watermark,
    /// otherwise it is the event count after which to wake up.
    ///
    /// ```c
    /// union {
    ///     /// wakeup every n events
    ///     __u32 wakeup_events;
    ///     /// bytes before wakeup
    ///     __u32 wakeup_watermark;
    /// };
    /// ```
    pub wakeup_events_or_watermark: u32,

    /// breakpoint type
    pub bp_type: HwBreakpointType,

    /// Union discriminator is ???
    ///
    /// ```c
    /// union {
    ///     __u64 bp_addr;
    ///     __u64 kprobe_func; /* for perf_kprobe */
    ///     __u64 uprobe_path; /* for perf_uprobe */
    ///     __u64 config1; /* extension of config */
    /// };
    /// ```
    pub bp_addr_or_kprobe_func_or_uprobe_func_or_config1: u64,

    /// Union discriminator is ???
    ///
    /// ```c
    /// union {
    ///     __u64 bp_len; /* breakpoint length, uses HW_BREAKPOINT_LEN_* constants */
    ///     __u64 kprobe_addr; /* when kprobe_func == NULL */
    ///     __u64 probe_offset; /* for perf_[k,u]probe */
    ///     __u64 config2; /* extension of config1 */
    /// };
    pub bp_len_or_kprobe_addr_or_probe_offset_or_config2: u64,

    /// Branch-sample specific flags.
    pub branch_sample_format: BranchSampleFormat,

    /// Defines set of user regs to dump on samples.
    /// See asm/perf_regs.h for details.
    pub sample_regs_user: u64,

    /// Defines size of the user stack to dump on samples.
    pub sample_stack_user: u32,

    /// The clock ID.
    pub clockid: ClockId,

    /// Defines set of regs to dump for each sample
    /// state captured on:
    ///  - precise = 0: PMU interrupt
    ///  - precise > 0: sampled instruction
    ///
    /// See asm/perf_regs.h for details.
    pub sample_regs_intr: u64,

    /// Wakeup watermark for AUX area
    pub aux_watermark: u32,

    /// When collecting stacks, this is the maximum number of stack frames
    /// (user + kernel) to collect.
    pub sample_max_stack: u16,

    /// When sampling AUX events, this is the size of the AUX sample.
    pub aux_sample_size: u32,

    /// User provided data if sigtrap=1, passed back to user via
    /// siginfo_t::si_perf_data, e.g. to permit user to identify the event.
    /// Note, siginfo_t::si_perf_data is long-sized, and sig_data will be
    /// truncated accordingly on 32 bit architectures.
    pub sig_data: u64,
}

impl PerfEventAttr {
    pub fn parse<R: Read, T: ByteOrder>(
        reader: &mut R,
        size: Option<u32>,
    ) -> Result<Self, std::io::Error> {
        let type_ = reader.read_u32::<T>()?;
        let self_described_size = reader.read_u32::<T>()?;
        let config = reader.read_u64::<T>()?;

        let size = size.unwrap_or(self_described_size);
        if size < PERF_ATTR_SIZE_VER0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }

        let sampling_period_or_frequency = reader.read_u64::<T>()?;
        let sample_type = reader.read_u64::<T>()?;
        let read_format = reader.read_u64::<T>()?;
        let flags = reader.read_u64::<T>()?;
        let wakeup_events_or_watermark = reader.read_u32::<T>()?;
        let bp_type = reader.read_u32::<T>()?;
        let bp_addr_or_kprobe_func_or_uprobe_func_or_config1 = reader.read_u64::<T>()?;

        let bp_len_or_kprobe_addr_or_probe_offset_or_config2 = if size >= PERF_ATTR_SIZE_VER1 {
            reader.read_u64::<T>()?
        } else {
            0
        };

        let branch_sample_type = if size >= PERF_ATTR_SIZE_VER2 {
            reader.read_u64::<T>()?
        } else {
            0
        };

        let (sample_regs_user, sample_stack_user, clockid) = if size >= PERF_ATTR_SIZE_VER3 {
            let sample_regs_user = reader.read_u64::<T>()?;
            let sample_stack_user = reader.read_u32::<T>()?;
            let clockid = reader.read_u32::<T>()?;

            (sample_regs_user, sample_stack_user, clockid)
        } else {
            (0, 0, 0)
        };

        let sample_regs_intr = if size >= PERF_ATTR_SIZE_VER4 {
            reader.read_u64::<T>()?
        } else {
            0
        };

        let (aux_watermark, sample_max_stack) = if size >= PERF_ATTR_SIZE_VER5 {
            let aux_watermark = reader.read_u32::<T>()?;
            let sample_max_stack = reader.read_u16::<T>()?;
            let __reserved_2 = reader.read_u16::<T>()?;
            (aux_watermark, sample_max_stack)
        } else {
            (0, 0)
        };

        let aux_sample_size = if size >= PERF_ATTR_SIZE_VER6 {
            let aux_sample_size = reader.read_u32::<T>()?;
            let __reserved_3 = reader.read_u32::<T>()?;
            aux_sample_size
        } else {
            0
        };

        let sig_data = if size >= PERF_ATTR_SIZE_VER7 {
            reader.read_u64::<T>()?
        } else {
            0
        };

        // Consume any remaining bytes.
        if size > PERF_ATTR_SIZE_VER7 {
            let remaining = size - PERF_ATTR_SIZE_VER7;
            io::copy(&mut reader.by_ref().take(remaining.into()), &mut io::sink())?;
        }

        Ok(Self {
            type_,
            size,
            config,
            sampling_period_or_frequency,
            sample_format: SampleFormat::from_bits_truncate(sample_type),
            read_format: ReadFormat::from_bits_truncate(read_format),
            flags: AttrFlags::from_bits_truncate(flags),
            wakeup_events_or_watermark,
            bp_type: HwBreakpointType::from_bits_truncate(bp_type),
            bp_addr_or_kprobe_func_or_uprobe_func_or_config1,
            bp_len_or_kprobe_addr_or_probe_offset_or_config2,
            branch_sample_format: BranchSampleFormat::from_bits_truncate(branch_sample_type),
            sample_regs_user,
            sample_stack_user,
            clockid: ClockId::from_u32(clockid).ok_or(io::ErrorKind::InvalidInput)?,
            sample_regs_intr,
            aux_watermark,
            sample_max_stack,
            aux_sample_size,
            sig_data,
        })
    }
}

pub struct RawEvent<'a> {
    pub record_type: RecordType,
    pub misc: u16,
    pub data: RawData<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEvent<'a> {
    pub id: Option<u64>,
    pub addr: Option<u64>,
    pub stream_id: Option<u64>,
    pub raw: Option<RawData<'a>>,
    pub ip: Option<u64>,
    pub timestamp: Option<u64>,
    pub pid: Option<i32>,
    pub tid: Option<i32>,
    pub cpu: Option<u32>,
    pub period: Option<u64>,
    pub user_regs: Option<Regs<'a>>,
    pub user_stack: Option<(RawData<'a>, u64)>,
    pub callchain: Option<RawData<'a>>,
    pub phys_addr: Option<u64>,
    pub data_page_size: Option<u64>,
    pub code_page_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regs<'a> {
    regs_mask: u64,
    raw_regs: RawRegs<'a>,
}

impl<'a> Regs<'a> {
    pub fn new(regs_mask: u64, raw_regs: RawRegs<'a>) -> Self {
        Self {
            regs_mask,
            raw_regs,
        }
    }

    pub fn get(&self, register: u64) -> Option<u64> {
        if self.regs_mask & (1 << register) == 0 {
            return None;
        }

        let mut index = 0;
        for i in 0..register {
            if self.regs_mask & (1 << i) != 0 {
                index += 1;
            }
        }
        Some(self.raw_regs.get(index))
    }
}

#[derive(Debug)]
pub struct ProcessEvent {
    pub pid: i32,
    pub ppid: i32,
    pub tid: i32,
    pub ptid: i32,
    pub timestamp: u64,
}

pub struct CommEvent<'a> {
    pub pid: i32,
    pub tid: i32,
    pub name: RawData<'a>,
    pub is_execve: bool,
}

/// These aren't emitted by the kernel any more - the kernel uses MMAP2 events
/// these days.
/// However, `perf record` still emits synthetic MMAP events (not MMAP2!) for
/// the kernel image. So if you want to symbolicate kernel addresses you still
/// need to process these.
/// The kernel image MMAP events have pid -1.
pub struct MmapEvent<'a> {
    pub pid: i32,
    pub tid: i32,
    pub address: u64,
    pub length: u64,
    pub page_offset: u64,
    pub is_executable: bool,
    pub cpu_mode: CpuMode,
    pub path: RawData<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuMode {
    Unknown,
    Kernel,
    User,
    Hypervisor,
    GuestKernel,
    GuestUser,
}

impl CpuMode {
    /// Initialize from the misc field of the perf event header.
    pub fn from_misc(misc: u16) -> Self {
        match misc & PERF_RECORD_MISC_CPUMODE_MASK {
            PERF_RECORD_MISC_CPUMODE_UNKNOWN => Self::Unknown,
            PERF_RECORD_MISC_KERNEL => Self::Kernel,
            PERF_RECORD_MISC_USER => Self::User,
            PERF_RECORD_MISC_HYPERVISOR => Self::Hypervisor,
            PERF_RECORD_MISC_GUEST_KERNEL => Self::GuestKernel,
            PERF_RECORD_MISC_GUEST_USER => Self::GuestUser,
            _ => Self::Unknown,
        }
    }
}
pub enum Mmap2FileId {
    InodeAndVersion(Mmap2InodeAndVersion),
    BuildId(Vec<u8>),
}

pub struct Mmap2Event<'a> {
    pub pid: i32,
    pub tid: i32,
    pub address: u64,
    pub length: u64,
    pub page_offset: u64,
    pub file_id: Mmap2FileId,
    pub protection: u32,
    pub flags: u32,
    pub cpu_mode: CpuMode,
    pub path: RawData<'a>,
}

pub struct Mmap2InodeAndVersion {
    pub major: u32,
    pub minor: u32,
    pub inode: u64,
    pub inode_generation: u64,
}

#[derive(Debug)]
pub struct LostEvent {
    pub id: u64,
    pub count: u64,
}

#[derive(Debug)]
pub struct ThrottleEvent {
    pub id: u64,
    pub timestamp: u64,
}

#[derive(Debug)]
pub enum ContextSwitchKind {
    In,
    OutWhileIdle,
    OutWhileRunning,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Event<'a> {
    Sample(SampleEvent<'a>),
    Comm(CommEvent<'a>),
    Exit(ProcessEvent),
    Fork(ProcessEvent),
    Mmap(MmapEvent<'a>),
    Mmap2(Mmap2Event<'a>),
    Lost(LostEvent),
    Throttle(ThrottleEvent),
    Unthrottle(ThrottleEvent),
    ContextSwitch(ContextSwitchKind),
    Raw(RawEvent<'a>),
}

impl<'a> fmt::Debug for CommEvent<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        use std::str;

        let mut map = fmt.debug_map();
        map.entry(&"pid", &self.pid).entry(&"tid", &self.tid);

        if let Ok(string) = str::from_utf8(&self.name.as_slice()) {
            map.entry(&"name", &string);
        } else {
            map.entry(&"name", &self.name);
        }

        map.finish()
    }
}

impl<'a> fmt::Debug for MmapEvent<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_map()
            .entry(&"pid", &self.pid)
            .entry(&"tid", &self.tid)
            .entry(&"address", &HexValue(self.address))
            .entry(&"length", &HexValue(self.length))
            .entry(&"page_offset", &HexValue(self.page_offset))
            .entry(&"cpu_mode", &self.cpu_mode)
            .entry(&"path", &&*String::from_utf8_lossy(&self.path.as_slice()))
            .finish()
    }
}

impl<'a> fmt::Debug for Mmap2Event<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_map()
            .entry(&"pid", &self.pid)
            .entry(&"tid", &self.tid)
            .entry(&"address", &HexValue(self.address))
            .entry(&"length", &HexValue(self.length))
            .entry(&"page_offset", &HexValue(self.page_offset))
            // .entry(&"major", &self.major)
            // .entry(&"minor", &self.minor)
            // .entry(&"inode", &self.inode)
            // .entry(&"inode_generation", &self.inode_generation)
            .entry(&"protection", &HexValue(self.protection as _))
            .entry(&"flags", &HexValue(self.flags as _))
            .entry(&"cpu_mode", &self.cpu_mode)
            .entry(&"path", &&*String::from_utf8_lossy(&self.path.as_slice()))
            .finish()
    }
}

impl<'a> fmt::Debug for RawEvent<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_map()
            .entry(&"record_type", &self.record_type)
            .entry(&"misc", &self.misc)
            .entry(&"data.len", &self.data.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordParseInfo {
    pub sample_format: SampleFormat,
    pub branch_sample_format: BranchSampleFormat,
    pub read_format: ReadFormat,
    pub common_data_offset_from_end: Option<usize>,
    pub sample_regs_user: u64,
    pub regs_count: usize,
    pub nonsample_record_time_offset_from_end: Option<usize>,
    pub nonsample_record_id_offset_from_end: Option<usize>,
    pub sample_record_time_offset_from_start: Option<usize>,
    pub sample_record_id_offset_from_start: Option<usize>,
}

impl RecordParseInfo {
    pub fn from_attr(attr: &PerfEventAttr) -> Self {
        let sample_format = attr.sample_format;
        let branch_sample_format = attr.branch_sample_format;
        let read_format = attr.read_format;

        // struct sample_id {
        //     { u32 pid, tid; }   /* if PERF_SAMPLE_TID set */
        //     { u64 time;     }   /* if PERF_SAMPLE_TIME set */
        //     { u64 id;       }   /* if PERF_SAMPLE_ID set */
        //     { u64 stream_id;}   /* if PERF_SAMPLE_STREAM_ID set  */
        //     { u32 cpu, res; }   /* if PERF_SAMPLE_CPU set */
        //     { u64 id;       }   /* if PERF_SAMPLE_IDENTIFIER set */
        // };
        let common_data_offset_from_end = if attr.flags.contains(AttrFlags::SAMPLE_ID_ALL) {
            Some(
                sample_format
                    .intersection(
                        SampleFormat::TID
                            | SampleFormat::TIME
                            | SampleFormat::ID
                            | SampleFormat::STREAM_ID
                            | SampleFormat::CPU
                            | SampleFormat::IDENTIFIER,
                    )
                    .bits
                    .count_ones() as usize
                    * 8,
            )
        } else {
            None
        };
        let sample_regs_user = attr.sample_regs_user;
        let regs_count = sample_regs_user.count_ones() as usize;
        let nonsample_record_time_offset_from_end = if attr.flags.contains(AttrFlags::SAMPLE_ID_ALL)
            && sample_format.contains(SampleFormat::TIME)
        {
            Some(
                sample_format
                    .intersection(
                        SampleFormat::TIME
                            | SampleFormat::ID
                            | SampleFormat::STREAM_ID
                            | SampleFormat::CPU
                            | SampleFormat::IDENTIFIER,
                    )
                    .bits
                    .count_ones() as usize
                    * 8,
            )
        } else {
            None
        };
        let nonsample_record_id_offset_from_end = if attr.flags.contains(AttrFlags::SAMPLE_ID_ALL)
            && sample_format.intersects(SampleFormat::ID | SampleFormat::IDENTIFIER)
        {
            if sample_format.contains(SampleFormat::IDENTIFIER) {
                Some(8)
            } else {
                Some(
                    sample_format
                        .intersection(
                            SampleFormat::ID
                                | SampleFormat::STREAM_ID
                                | SampleFormat::CPU
                                | SampleFormat::IDENTIFIER,
                        )
                        .bits
                        .count_ones() as usize
                        * 8,
                )
            }
        } else {
            None
        };

        // { u64 id;           } && PERF_SAMPLE_IDENTIFIER
        // { u64 ip;           } && PERF_SAMPLE_IP
        // { u32 pid; u32 tid; } && PERF_SAMPLE_TID
        // { u64 time;         } && PERF_SAMPLE_TIME
        // { u64 addr;         } && PERF_SAMPLE_ADDR
        // { u64 id;           } && PERF_SAMPLE_ID
        let sample_record_id_offset_from_start = if sample_format.contains(SampleFormat::IDENTIFIER)
        {
            Some(0)
        } else if sample_format.contains(SampleFormat::ID) {
            Some(
                sample_format
                    .intersection(
                        SampleFormat::IP
                            | SampleFormat::TID
                            | SampleFormat::TIME
                            | SampleFormat::ADDR,
                    )
                    .bits
                    .count_ones() as usize
                    * 8,
            )
        } else {
            None
        };
        let sample_record_time_offset_from_start = if sample_format.contains(SampleFormat::TIME) {
            Some(
                sample_format
                    .intersection(SampleFormat::IDENTIFIER | SampleFormat::IP | SampleFormat::TID)
                    .bits
                    .count_ones() as usize
                    * 8,
            )
        } else {
            None
        };

        Self {
            sample_format,
            branch_sample_format,
            read_format,
            common_data_offset_from_end,
            sample_regs_user,
            regs_count,
            nonsample_record_time_offset_from_end,
            nonsample_record_id_offset_from_end,
            sample_record_time_offset_from_start,
            sample_record_id_offset_from_start,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordType(pub u32);

impl RecordType {
    // Kernel-built-in record types
    pub const MMAP: Self = Self(PERF_RECORD_MMAP);
    pub const LOST: Self = Self(PERF_RECORD_LOST);
    pub const COMM: Self = Self(PERF_RECORD_COMM);
    pub const EXIT: Self = Self(PERF_RECORD_EXIT);
    pub const THROTTLE: Self = Self(PERF_RECORD_THROTTLE);
    pub const UNTHROTTLE: Self = Self(PERF_RECORD_UNTHROTTLE);
    pub const FORK: Self = Self(PERF_RECORD_FORK);
    pub const READ: Self = Self(PERF_RECORD_READ);
    pub const SAMPLE: Self = Self(PERF_RECORD_SAMPLE);
    pub const MMAP2: Self = Self(PERF_RECORD_MMAP2);
    pub const AUX: Self = Self(PERF_RECORD_AUX);
    pub const ITRACE_START: Self = Self(PERF_RECORD_ITRACE_START);
    pub const LOST_SAMPLES: Self = Self(PERF_RECORD_LOST_SAMPLES);
    pub const SWITCH: Self = Self(PERF_RECORD_SWITCH);
    pub const SWITCH_CPU_WIDE: Self = Self(PERF_RECORD_SWITCH_CPU_WIDE);
    pub const NAMESPACES: Self = Self(PERF_RECORD_NAMESPACES);
    pub const KSYMBOL: Self = Self(PERF_RECORD_KSYMBOL);
    pub const BPF_EVENT: Self = Self(PERF_RECORD_BPF_EVENT);
    pub const CGROUP: Self = Self(PERF_RECORD_CGROUP);
    pub const TEXT_POKE: Self = Self(PERF_RECORD_TEXT_POKE);
    pub const AUX_OUTPUT_HW_ID: Self = Self(PERF_RECORD_AUX_OUTPUT_HW_ID);

    // Record types added by the `perf` tool from user space
    pub const HEADER_ATTR: Self = Self(PERF_RECORD_HEADER_ATTR);
    pub const HEADER_EVENT_TYPE: Self = Self(PERF_RECORD_HEADER_EVENT_TYPE);
    pub const HEADER_TRACING_DATA: Self = Self(PERF_RECORD_HEADER_TRACING_DATA);
    pub const HEADER_BUILD_ID: Self = Self(PERF_RECORD_HEADER_BUILD_ID);
    pub const FINISHED_ROUND: Self = Self(PERF_RECORD_FINISHED_ROUND);
    pub const ID_INDEX: Self = Self(PERF_RECORD_ID_INDEX);
    pub const AUXTRACE_INFO: Self = Self(PERF_RECORD_AUXTRACE_INFO);
    pub const AUXTRACE: Self = Self(PERF_RECORD_AUXTRACE);
    pub const AUXTRACE_ERROR: Self = Self(PERF_RECORD_AUXTRACE_ERROR);
    pub const THREAD_MAP: Self = Self(PERF_RECORD_THREAD_MAP);
    pub const CPU_MAP: Self = Self(PERF_RECORD_CPU_MAP);
    pub const STAT_CONFIG: Self = Self(PERF_RECORD_STAT_CONFIG);
    pub const STAT: Self = Self(PERF_RECORD_STAT);
    pub const STAT_ROUND: Self = Self(PERF_RECORD_STAT_ROUND);
    pub const EVENT_UPDATE: Self = Self(PERF_RECORD_EVENT_UPDATE);
    pub const TIME_CONV: Self = Self(PERF_RECORD_TIME_CONV);
    pub const HEADER_FEATURE: Self = Self(PERF_RECORD_HEADER_FEATURE);
    pub const COMPRESSED: Self = Self(PERF_RECORD_COMPRESSED);

    pub fn is_builtin_type(&self) -> bool {
        self.0 < PERF_RECORD_USER_TYPE_START
    }

    pub fn is_user_type(&self) -> bool {
        self.0 >= PERF_RECORD_USER_TYPE_START
    }
}

impl fmt::Debug for RecordType {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let s = match *self {
            Self::MMAP => "MMAP",
            Self::LOST => "LOST",
            Self::COMM => "COMM",
            Self::EXIT => "EXIT",
            Self::THROTTLE => "THROTTLE",
            Self::UNTHROTTLE => "UNTHROTTLE",
            Self::FORK => "FORK",
            Self::READ => "READ",
            Self::SAMPLE => "SAMPLE",
            Self::MMAP2 => "MMAP2",
            Self::AUX => "AUX",
            Self::ITRACE_START => "ITRACE_START",
            Self::LOST_SAMPLES => "LOST_SAMPLES",
            Self::SWITCH => "SWITCH",
            Self::SWITCH_CPU_WIDE => "SWITCH_CPU_WIDE",
            Self::NAMESPACES => "NAMESPACES",
            Self::KSYMBOL => "KSYMBOL",
            Self::BPF_EVENT => "BPF_EVENT",
            Self::CGROUP => "CGROUP",
            Self::TEXT_POKE => "TEXT_POKE",
            Self::AUX_OUTPUT_HW_ID => "AUX_OUTPUT_HW_ID",
            Self::HEADER_ATTR => "HEADER_ATTR",
            Self::HEADER_EVENT_TYPE => "HEADER_EVENT_TYPE",
            Self::HEADER_TRACING_DATA => "HEADER_TRACING_DATA",
            Self::HEADER_BUILD_ID => "HEADER_BUILD_ID",
            Self::FINISHED_ROUND => "FINISHED_ROUND",
            Self::ID_INDEX => "ID_INDEX",
            Self::AUXTRACE_INFO => "AUXTRACE_INFO",
            Self::AUXTRACE => "AUXTRACE",
            Self::AUXTRACE_ERROR => "AUXTRACE_ERROR",
            Self::THREAD_MAP => "THREAD_MAP",
            Self::CPU_MAP => "CPU_MAP",
            Self::STAT_CONFIG => "STAT_CONFIG",
            Self::STAT => "STAT",
            Self::STAT_ROUND => "STAT_ROUND",
            Self::EVENT_UPDATE => "EVENT_UPDATE",
            Self::TIME_CONV => "TIME_CONV",
            Self::HEADER_FEATURE => "HEADER_FEATURE",
            Self::COMPRESSED => "COMPRESSED",
            other => {
                return fmt.write_fmt(format_args!("Unknown: {}", other.0));
            }
        };
        fmt.write_str(s)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommonData {
    pub pid: Option<i32>,
    pub tid: Option<i32>,
    pub timestamp: Option<u64>,
    pub id: Option<u64>,
    pub stream_id: Option<u64>,
    pub cpu: Option<u32>,
}

impl<'a> RawEvent<'a> {
    pub fn common_data<T: ByteOrder>(
        &self,
        parse_info: &RecordParseInfo,
    ) -> Result<CommonData, std::io::Error> {
        if self.record_type.is_user_type() {
            return Ok(Default::default());
        }

        let sample_format = parse_info.sample_format;

        if self.record_type == RecordType::SAMPLE {
            // { u64 id;       } && PERF_SAMPLE_IDENTIFIER
            // { u64 ip;       } && PERF_SAMPLE_IP
            // { u32 pid, tid; } && PERF_SAMPLE_TID
            // { u64 time;     } && PERF_SAMPLE_TIME
            // { u64 addr;     } && PERF_SAMPLE_ADDR
            // { u64 id;       } && PERF_SAMPLE_ID
            // { u64 stream_id;} && PERF_SAMPLE_STREAM_ID
            // { u32 cpu, res; } && PERF_SAMPLE_CPU
            let mut cur = self.data;
            let identifier = if sample_format.contains(SampleFormat::IDENTIFIER) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            if sample_format.contains(SampleFormat::IP) {
                let _ip = cur.read_u64::<T>()?;
            }

            let (pid, tid) = if sample_format.contains(SampleFormat::TID) {
                let pid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                (Some(pid), Some(tid))
            } else {
                (None, None)
            };

            let timestamp = if sample_format.contains(SampleFormat::TIME) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            if sample_format.contains(SampleFormat::ADDR) {
                let _addr = cur.read_u64::<T>()?;
            }

            let id = if sample_format.contains(SampleFormat::ID) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };
            let id = identifier.or(id);

            let stream_id = if sample_format.contains(SampleFormat::STREAM_ID) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            let cpu = if sample_format.contains(SampleFormat::CPU) {
                let cpu = cur.read_u32::<T>()?;
                let _ = cur.read_u32::<T>()?; // Reserved field; is always zero.
                Some(cpu)
            } else {
                None
            };

            Ok(CommonData {
                pid,
                tid,
                timestamp,
                id,
                stream_id,
                cpu,
            })
        } else if let Some(common_data_offset_from_end) = parse_info.common_data_offset_from_end {
            let mut cur = self.data;
            let common_data_offset_from_start = cur
                .len()
                .checked_sub(common_data_offset_from_end)
                .ok_or(std::io::ErrorKind::UnexpectedEof)?;
            cur.skip(common_data_offset_from_start)?;

            // struct sample_id {
            //     { u32 pid, tid;  }   /* if PERF_SAMPLE_TID set */
            //     { u64 timestamp; }   /* if PERF_SAMPLE_TIME set */
            //     { u64 id;        }   /* if PERF_SAMPLE_ID set */
            //     { u64 stream_id; }   /* if PERF_SAMPLE_STREAM_ID set  */
            //     { u32 cpu, res;  }   /* if PERF_SAMPLE_CPU set */
            //     { u64 identifier;}   /* if PERF_SAMPLE_IDENTIFIER set */
            // };
            let (pid, tid) = if sample_format.contains(SampleFormat::TID) {
                let pid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                (Some(pid), Some(tid))
            } else {
                (None, None)
            };

            let timestamp = if sample_format.contains(SampleFormat::TIME) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            let id = if sample_format.contains(SampleFormat::ID) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            let stream_id = if sample_format.contains(SampleFormat::STREAM_ID) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };

            let cpu = if sample_format.contains(SampleFormat::CPU) {
                let cpu = cur.read_u32::<T>()?;
                let _ = cur.read_u32::<T>()?; // Reserved field; is always zero.
                Some(cpu)
            } else {
                None
            };

            let identifier = if sample_format.contains(SampleFormat::IDENTIFIER) {
                Some(cur.read_u64::<T>()?)
            } else {
                None
            };
            let id = identifier.or(id);

            Ok(CommonData {
                pid,
                tid,
                timestamp,
                id,
                stream_id,
                cpu,
            })
        } else {
            Ok(Default::default())
        }
    }

    pub fn timestamp<T: ByteOrder>(&self, parse_info: &RecordParseInfo) -> Option<u64> {
        if self.record_type.is_user_type() {
            return None;
        }

        if self.record_type == RecordType::SAMPLE {
            if let Some(time_offset_from_start) = parse_info.sample_record_time_offset_from_start {
                let mut data = self.data;
                data.skip(time_offset_from_start).ok()?;
                data.read_u64::<T>().ok()
            } else {
                None
            }
        } else if let Some(time_offset_from_end) = parse_info.nonsample_record_time_offset_from_end
        {
            let mut data = self.data;
            let time_offset_from_start = data.len().checked_sub(time_offset_from_end)?;
            data.skip(time_offset_from_start).ok()?;
            data.read_u64::<T>().ok()
        } else {
            None
        }
    }

    pub fn id<T: ByteOrder>(&self, parse_info: &RecordParseInfo) -> Option<u64> {
        if self.record_type.is_user_type() {
            return None;
        }

        if self.record_type == RecordType::SAMPLE {
            if let Some(id_offset_from_start) = parse_info.sample_record_id_offset_from_start {
                let mut data = self.data;
                data.skip(id_offset_from_start).ok()?;
                data.read_u64::<T>().ok()
            } else {
                None
            }
        } else if let Some(id_offset_from_end) = parse_info.nonsample_record_id_offset_from_end {
            let mut data = self.data;
            let id_offset_from_start = data.len().checked_sub(id_offset_from_end)?;
            data.skip(id_offset_from_start).ok()?;
            data.read_u64::<T>().ok()
        } else {
            None
        }
    }

    pub fn parse<T: ByteOrder>(
        self,
        parse_info: &RecordParseInfo,
    ) -> Result<Event<'a>, std::io::Error> {
        let sample_format = parse_info.sample_format;
        let branch_sample_format = parse_info.branch_sample_format;
        let read_format = parse_info.read_format;
        let regs_count = parse_info.regs_count;
        let sample_regs_user = parse_info.sample_regs_user;
        let mut cur = self.data;
        let event = match self.record_type {
            RecordType::EXIT | RecordType::FORK => {
                let pid = cur.read_i32::<T>()?;
                let ppid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                let ptid = cur.read_i32::<T>()?;
                let timestamp = cur.read_u64::<T>()?;

                let event = ProcessEvent {
                    pid,
                    ppid,
                    tid,
                    ptid,
                    timestamp,
                };

                if self.record_type == RecordType::EXIT {
                    Event::Exit(event)
                } else {
                    Event::Fork(event)
                }
            }

            RecordType::SAMPLE => {
                let identifier = if sample_format.contains(SampleFormat::IDENTIFIER) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let ip = if sample_format.contains(SampleFormat::IP) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let (pid, tid) = if sample_format.contains(SampleFormat::TID) {
                    let pid = cur.read_i32::<T>()?;
                    let tid = cur.read_i32::<T>()?;
                    (Some(pid), Some(tid))
                } else {
                    (None, None)
                };

                let timestamp = if sample_format.contains(SampleFormat::TIME) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let addr = if sample_format.contains(SampleFormat::ADDR) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let id = if sample_format.contains(SampleFormat::ID) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };
                let id = identifier.or(id);

                let stream_id = if sample_format.contains(SampleFormat::STREAM_ID) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let cpu = if sample_format.contains(SampleFormat::CPU) {
                    let cpu = cur.read_u32::<T>()?;
                    let _reserved = cur.read_u32::<T>()?;
                    Some(cpu)
                } else {
                    None
                };

                let period = if sample_format.contains(SampleFormat::PERIOD) {
                    let period = cur.read_u64::<T>()?;
                    Some(period)
                } else {
                    None
                };

                if sample_format.contains(SampleFormat::READ) {
                    if read_format.contains(ReadFormat::GROUP) {
                        let _value = cur.read_u64::<T>()?;
                        if read_format.contains(ReadFormat::TOTAL_TIME_ENABLED) {
                            let _time_enabled = cur.read_u64::<T>()?;
                        }
                        if read_format.contains(ReadFormat::TOTAL_TIME_RUNNING) {
                            let _time_running = cur.read_u64::<T>()?;
                        }
                        if read_format.contains(ReadFormat::ID) {
                            let _id = cur.read_u64::<T>()?;
                        }
                    } else {
                        let nr = cur.read_u64::<T>()?;
                        if read_format.contains(ReadFormat::TOTAL_TIME_ENABLED) {
                            let _time_enabled = cur.read_u64::<T>()?;
                        }
                        if read_format.contains(ReadFormat::TOTAL_TIME_RUNNING) {
                            let _time_running = cur.read_u64::<T>()?;
                        }
                        for _ in 0..nr {
                            let _value = cur.read_u64::<T>()?;
                            if read_format.contains(ReadFormat::ID) {
                                let _id = cur.read_u64::<T>()?;
                            }
                        }
                    }
                }

                let callchain =
                    if sample_format.contains(SampleFormat::CALLCHAIN) {
                        let callchain_length = cur.read_u64::<T>()?;
                        Some(cur.split_off_prefix(
                            callchain_length as usize * std::mem::size_of::<u64>(),
                        )?)
                    } else {
                        None
                    };

                let raw = if sample_format.contains(SampleFormat::RAW) {
                    let size = cur.read_u32::<T>()?;
                    Some(cur.split_off_prefix(size as usize)?)
                } else {
                    None
                };

                if sample_format.contains(SampleFormat::BRANCH_STACK) {
                    let nr = cur.read_u64::<T>()?;
                    if branch_sample_format.contains(BranchSampleFormat::HW_INDEX) {
                        let _hw_idx = cur.read_u64::<T>()?;
                    }
                    for _ in 0..nr {
                        let _from = cur.read_u64::<T>()?;
                        let _to = cur.read_u64::<T>()?;
                        let _flags = cur.read_u64::<T>()?;
                    }
                }

                let user_regs = if sample_format.contains(SampleFormat::REGS_USER) {
                    let regs_abi = cur.read_u64::<T>()?;
                    if regs_abi == 0 {
                        None
                    } else {
                        let raw_regs =
                            cur.split_off_prefix(regs_count * std::mem::size_of::<u64>())?;
                        let raw_regs = RawRegs::from_raw_data(raw_regs);
                        let user_regs = Regs::new(sample_regs_user, raw_regs);
                        Some(user_regs)
                    }
                } else {
                    None
                };

                let user_stack = if sample_format.contains(SampleFormat::STACK_USER) {
                    let stack_size = cur.read_u64::<T>()?;
                    let stack = cur.split_off_prefix(stack_size as usize)?;

                    let dynamic_size = if stack_size != 0 {
                        cur.read_u64::<T>()?
                    } else {
                        0
                    };
                    Some((stack, dynamic_size))
                } else {
                    None
                };

                if sample_format.contains(SampleFormat::WEIGHT) {
                    let _weight = cur.read_u64::<T>()?;
                }

                if sample_format.contains(SampleFormat::DATA_SRC) {
                    let _data_src = cur.read_u64::<T>()?;
                }

                if sample_format.contains(SampleFormat::TRANSACTION) {
                    let _transaction = cur.read_u64::<T>()?;
                }

                if sample_format.contains(SampleFormat::REGS_INTR) {
                    let regs_abi = cur.read_u64::<T>()?;
                    if regs_abi != 0 {
                        cur.skip(regs_count * std::mem::size_of::<u64>())?;
                    }
                }

                let phys_addr = if sample_format.contains(SampleFormat::PHYS_ADDR) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                if sample_format.contains(SampleFormat::AUX) {
                    let size = cur.read_u64::<T>()?;
                    cur.skip(size as usize)?;
                }

                let data_page_size = if sample_format.contains(SampleFormat::DATA_PAGE_SIZE) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                let code_page_size = if sample_format.contains(SampleFormat::CODE_PAGE_SIZE) {
                    Some(cur.read_u64::<T>()?)
                } else {
                    None
                };

                Event::Sample(SampleEvent {
                    id,
                    ip,
                    addr,
                    stream_id,
                    raw,
                    user_regs,
                    user_stack,
                    callchain,
                    cpu,
                    timestamp,
                    pid,
                    tid,
                    period,
                    phys_addr,
                    data_page_size,
                    code_page_size,
                })
            }

            RecordType::COMM => {
                let pid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                let name = cur.read_string().unwrap_or(cur); // TODO: return error if no string terminator found

                // TODO: Maybe feature-gate this on 3.16+
                let is_execve = self.misc & PERF_RECORD_MISC_COMM_EXEC != 0;

                Event::Comm(CommEvent {
                    pid,
                    tid,
                    name,
                    is_execve,
                })
            }

            RecordType::MMAP => {
                // struct {
                //   struct perf_event_header header;
                //
                //   u32 pid, tid;
                //   u64 addr;
                //   u64 len;
                //   u64 pgoff;
                //   char filename[];
                //   struct sample_id sample_id;
                // };

                let pid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                let address = cur.read_u64::<T>()?;
                let length = cur.read_u64::<T>()?;
                let page_offset = cur.read_u64::<T>()?;
                let path = cur.read_string().unwrap_or(cur); // TODO: return error if no string terminator found
                let is_executable = self.misc & PERF_RECORD_MISC_MMAP_DATA == 0;

                Event::Mmap(MmapEvent {
                    pid,
                    tid,
                    address,
                    length,
                    page_offset,
                    is_executable,
                    cpu_mode: CpuMode::from_misc(self.misc),
                    path,
                })
            }

            RecordType::MMAP2 => {
                let pid = cur.read_i32::<T>()?;
                let tid = cur.read_i32::<T>()?;
                let address = cur.read_u64::<T>()?;
                let length = cur.read_u64::<T>()?;
                let page_offset = cur.read_u64::<T>()?;
                let file_id = if self.misc & PERF_RECORD_MISC_MMAP_BUILD_ID != 0 {
                    let build_id_len = cur.read_u8()?;
                    assert!(build_id_len <= 20);
                    let _align = cur.read_u8()?;
                    let _align = cur.read_u16::<T>()?;
                    let mut build_id_bytes = [0; 20];
                    cur.read_exact(&mut build_id_bytes)?;
                    Mmap2FileId::BuildId(build_id_bytes[..build_id_len as usize].to_owned())
                } else {
                    let major = cur.read_u32::<T>()?;
                    let minor = cur.read_u32::<T>()?;
                    let inode = cur.read_u64::<T>()?;
                    let inode_generation = cur.read_u64::<T>()?;
                    Mmap2FileId::InodeAndVersion(Mmap2InodeAndVersion {
                        major,
                        minor,
                        inode,
                        inode_generation,
                    })
                };
                let protection = cur.read_u32::<T>()?;
                let flags = cur.read_u32::<T>()?;
                let path = cur.read_string().unwrap_or(cur); // TODO: return error if no string terminator found

                Event::Mmap2(Mmap2Event {
                    pid,
                    tid,
                    address,
                    length,
                    page_offset,
                    file_id,
                    protection,
                    flags,
                    cpu_mode: CpuMode::from_misc(self.misc),
                    path,
                })
            }

            RecordType::LOST => {
                let id = cur.read_u64::<T>()?;
                let count = cur.read_u64::<T>()?;
                Event::Lost(LostEvent { id, count })
            }

            RecordType::THROTTLE | RecordType::UNTHROTTLE => {
                let timestamp = cur.read_u64::<T>()?;
                let id = cur.read_u64::<T>()?;
                let event = ThrottleEvent { id, timestamp };
                if self.record_type == RecordType::THROTTLE {
                    Event::Throttle(event)
                } else {
                    Event::Unthrottle(event)
                }
            }

            RecordType::SWITCH => {
                let is_out = self.misc & PERF_RECORD_MISC_SWITCH_OUT != 0;
                let is_out_preempt = self.misc & PERF_RECORD_MISC_SWITCH_OUT_PREEMPT != 0;
                let kind = if is_out {
                    if is_out_preempt {
                        ContextSwitchKind::OutWhileRunning
                    } else {
                        ContextSwitchKind::OutWhileIdle
                    }
                } else {
                    ContextSwitchKind::In
                };

                Event::ContextSwitch(kind)
            }

            _ => Event::Raw(self),
        };
        Ok(event)
    }
}
