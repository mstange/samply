#![allow(unused)]

use std::fmt;

use libc::{c_int, c_ulong, pid_t, syscall, SYS_perf_event_open};

#[cfg(target_endian = "big")]
macro_rules! flag {
    ($nth:expr) => {
        (1 << 63) >> $nth
    };
}

#[cfg(target_endian = "little")]
macro_rules! flag {
    ($nth:expr) => {
        1 << $nth
    };
}

pub const PERF_FLAG_FD_CLOEXEC: c_ulong = 1 << 3;

pub const PERF_TYPE_HARDWARE: u32 = 0;
pub const PERF_TYPE_SOFTWARE: u32 = 1;
pub const PERF_TYPE_TRACEPOINT: u32 = 2;

pub const PERF_ATTR_FLAG_DISABLED: u64 = flag!(0);
pub const PERF_ATTR_FLAG_INHERIT: u64 = flag!(1);
pub const PERF_ATTR_FLAG_PINNED: u64 = flag!(2);
pub const PERF_ATTR_FLAG_EXCLUSIVE: u64 = flag!(3);
pub const PERF_ATTR_FLAG_EXCLUDE_USER: u64 = flag!(4);
pub const PERF_ATTR_FLAG_EXCLUDE_KERNEL: u64 = flag!(5);
pub const PERF_ATTR_FLAG_EXCLUDE_HV: u64 = flag!(6);
pub const PERF_ATTR_FLAG_EXCLUDE_IDLE: u64 = flag!(7);
pub const PERF_ATTR_FLAG_MMAP: u64 = flag!(8);
pub const PERF_ATTR_FLAG_COMM: u64 = flag!(9);
pub const PERF_ATTR_FLAG_FREQ: u64 = flag!(10);
pub const PERF_ATTR_FLAG_INHERIT_STAT: u64 = flag!(11);
pub const PERF_ATTR_FLAG_ENABLE_ON_EXEC: u64 = flag!(12);
pub const PERF_ATTR_FLAG_TASK: u64 = flag!(13);
pub const PERF_ATTR_FLAG_WATERMARK: u64 = flag!(14);
pub const PERF_ATTR_FLAG_MMAP_DATA: u64 = flag!(17);
pub const PERF_ATTR_FLAG_SAMPLE_ID_ALL: u64 = flag!(18);
pub const PERF_ATTR_FLAG_EXCLUDE_HOST: u64 = flag!(19);
pub const PERF_ATTR_FLAG_EXCLUDE_GUEST: u64 = flag!(20);
pub const PERF_ATTR_FLAG_EXCLUDE_CALLCHAIN_KERNEL: u64 = flag!(21);
pub const PERF_ATTR_FLAG_EXCLUDE_CALLCHAIN_USER: u64 = flag!(22);
pub const PERF_ATTR_FLAG_MMAP2: u64 = flag!(23);
pub const PERF_ATTR_FLAG_COMM_EXEC: u64 = flag!(24);
pub const PERF_ATTR_FLAG_USE_CLOCKID: u64 = flag!(25);
pub const PERF_ATTR_FLAG_CONTEX_SWITCH: u64 = flag!(26);

pub const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
pub const PERF_COUNT_HW_REF_CPU_CYCLES: u64 = 9;

pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
pub const PERF_COUNT_SW_TASK_CLOCK: u64 = 1;
pub const PERF_COUNT_SW_PAGE_FAULTS: u64 = 2;
pub const PERF_COUNT_SW_DUMMY: u64 = 9;

pub const PERF_RECORD_LOST: u32 = 2;
pub const PERF_RECORD_COMM: u32 = 3;
pub const PERF_RECORD_EXIT: u32 = 4;
pub const PERF_RECORD_THROTTLE: u32 = 5;
pub const PERF_RECORD_UNTHROTTLE: u32 = 6;
pub const PERF_RECORD_FORK: u32 = 7;
pub const PERF_RECORD_SAMPLE: u32 = 9;
pub const PERF_RECORD_MMAP2: u32 = 10;
pub const PERF_RECORD_SWITCH: u32 = 14;

pub const PERF_RECORD_MISC_SWITCH_OUT: u16 = 1 << 13;
pub const PERF_RECORD_MISC_SWITCH_OUT_PREEMPT: u16 = 1 << 14;

pub const PERF_SAMPLE_IP: u64 = 1 << 0;
pub const PERF_SAMPLE_TID: u64 = 1 << 1;
pub const PERF_SAMPLE_TIME: u64 = 1 << 2;
pub const PERF_SAMPLE_ADDR: u64 = 1 << 3;
pub const PERF_SAMPLE_READ: u64 = 1 << 4;
pub const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
pub const PERF_SAMPLE_ID: u64 = 1 << 6;
pub const PERF_SAMPLE_CPU: u64 = 1 << 7;
pub const PERF_SAMPLE_PERIOD: u64 = 1 << 8;
pub const PERF_SAMPLE_STREAM_ID: u64 = 1 << 9;
pub const PERF_SAMPLE_RAW: u64 = 1 << 10;
pub const PERF_SAMPLE_BRANCH_STACK: u64 = 1 << 11;
pub const PERF_SAMPLE_REGS_USER: u64 = 1 << 12;
pub const PERF_SAMPLE_STACK_USER: u64 = 1 << 13;
pub const PERF_SAMPLE_WEIGHT: u64 = 1 << 14;
pub const PERF_SAMPLE_DATA_SRC: u64 = 1 << 15;
pub const PERF_SAMPLE_IDENTIFIER: u64 = 1 << 16;
pub const PERF_SAMPLE_TRANSACTION: u64 = 1 << 17;
pub const PERF_SAMPLE_REGS_INTR: u64 = 1 << 18;

pub const PERF_REG_X86_AX: u64 = 0;
pub const PERF_REG_X86_BX: u64 = 1;
pub const PERF_REG_X86_CX: u64 = 2;
pub const PERF_REG_X86_DX: u64 = 3;
pub const PERF_REG_X86_SI: u64 = 4;
pub const PERF_REG_X86_DI: u64 = 5;
pub const PERF_REG_X86_BP: u64 = 6;
pub const PERF_REG_X86_SP: u64 = 7;
pub const PERF_REG_X86_IP: u64 = 8;
pub const PERF_REG_X86_FLAGS: u64 = 9;
pub const PERF_REG_X86_CS: u64 = 10;
pub const PERF_REG_X86_SS: u64 = 11;
pub const PERF_REG_X86_DS: u64 = 12;
pub const PERF_REG_X86_ES: u64 = 13;
pub const PERF_REG_X86_FS: u64 = 14;
pub const PERF_REG_X86_GS: u64 = 15;
pub const PERF_REG_X86_R8: u64 = 16;
pub const PERF_REG_X86_R9: u64 = 17;
pub const PERF_REG_X86_R10: u64 = 18;
pub const PERF_REG_X86_R11: u64 = 19;
pub const PERF_REG_X86_R12: u64 = 20;
pub const PERF_REG_X86_R13: u64 = 21;
pub const PERF_REG_X86_R14: u64 = 22;
pub const PERF_REG_X86_R15: u64 = 23;

pub const PERF_REG_X86_32_MAX: u64 = PERF_REG_X86_GS + 1;
pub const PERF_REG_X86_64_MAX: u64 = PERF_REG_X86_R15 + 1;

pub const PERF_REG_ARM_R0: u64 = 0;
pub const PERF_REG_ARM_R1: u64 = 1;
pub const PERF_REG_ARM_R2: u64 = 2;
pub const PERF_REG_ARM_R3: u64 = 3;
pub const PERF_REG_ARM_R4: u64 = 4;
pub const PERF_REG_ARM_R5: u64 = 5;
pub const PERF_REG_ARM_R6: u64 = 6;
pub const PERF_REG_ARM_R7: u64 = 7;
pub const PERF_REG_ARM_R8: u64 = 8;
pub const PERF_REG_ARM_R9: u64 = 9;
pub const PERF_REG_ARM_R10: u64 = 10;
pub const PERF_REG_ARM_FP: u64 = 11;
pub const PERF_REG_ARM_IP: u64 = 12;
pub const PERF_REG_ARM_SP: u64 = 13;
pub const PERF_REG_ARM_LR: u64 = 14;
pub const PERF_REG_ARM_PC: u64 = 15;
pub const PERF_REG_ARM_MAX: u64 = 16;

pub const PERF_REG_MIPS_PC: u64 = 0;
pub const PERF_REG_MIPS_R1: u64 = 1;
pub const PERF_REG_MIPS_R2: u64 = 2;
pub const PERF_REG_MIPS_R3: u64 = 3;
pub const PERF_REG_MIPS_R4: u64 = 4;
pub const PERF_REG_MIPS_R5: u64 = 5;
pub const PERF_REG_MIPS_R6: u64 = 6;
pub const PERF_REG_MIPS_R7: u64 = 7;
pub const PERF_REG_MIPS_R8: u64 = 8;
pub const PERF_REG_MIPS_R9: u64 = 9;
pub const PERF_REG_MIPS_R10: u64 = 10;
pub const PERF_REG_MIPS_R11: u64 = 11;
pub const PERF_REG_MIPS_R12: u64 = 12;
pub const PERF_REG_MIPS_R13: u64 = 13;
pub const PERF_REG_MIPS_R14: u64 = 14;
pub const PERF_REG_MIPS_R15: u64 = 15;
pub const PERF_REG_MIPS_R16: u64 = 16;
pub const PERF_REG_MIPS_R17: u64 = 17;
pub const PERF_REG_MIPS_R18: u64 = 18;
pub const PERF_REG_MIPS_R19: u64 = 19;
pub const PERF_REG_MIPS_R20: u64 = 20;
pub const PERF_REG_MIPS_R21: u64 = 21;
pub const PERF_REG_MIPS_R22: u64 = 22;
pub const PERF_REG_MIPS_R23: u64 = 23;
pub const PERF_REG_MIPS_R24: u64 = 24;
pub const PERF_REG_MIPS_R25: u64 = 25;
pub const PERF_REG_MIPS_R28: u64 = 26;
pub const PERF_REG_MIPS_R29: u64 = 27;
pub const PERF_REG_MIPS_R30: u64 = 28;
pub const PERF_REG_MIPS_R31: u64 = 29;
pub const PERF_REG_MIPS_MAX: u64 = PERF_REG_MIPS_R31 + 1;

pub const PERF_REG_ARM64_X0: u64 = 0;
pub const PERF_REG_ARM64_X1: u64 = 1;
pub const PERF_REG_ARM64_X2: u64 = 2;
pub const PERF_REG_ARM64_X3: u64 = 3;
pub const PERF_REG_ARM64_X4: u64 = 4;
pub const PERF_REG_ARM64_X5: u64 = 5;
pub const PERF_REG_ARM64_X6: u64 = 6;
pub const PERF_REG_ARM64_X7: u64 = 7;
pub const PERF_REG_ARM64_X8: u64 = 8;
pub const PERF_REG_ARM64_X9: u64 = 9;
pub const PERF_REG_ARM64_X10: u64 = 10;
pub const PERF_REG_ARM64_X11: u64 = 11;
pub const PERF_REG_ARM64_X12: u64 = 12;
pub const PERF_REG_ARM64_X13: u64 = 13;
pub const PERF_REG_ARM64_X14: u64 = 14;
pub const PERF_REG_ARM64_X15: u64 = 15;
pub const PERF_REG_ARM64_X16: u64 = 16;
pub const PERF_REG_ARM64_X17: u64 = 17;
pub const PERF_REG_ARM64_X18: u64 = 18;
pub const PERF_REG_ARM64_X19: u64 = 19;
pub const PERF_REG_ARM64_X20: u64 = 20;
pub const PERF_REG_ARM64_X21: u64 = 21;
pub const PERF_REG_ARM64_X22: u64 = 22;
pub const PERF_REG_ARM64_X23: u64 = 23;
pub const PERF_REG_ARM64_X24: u64 = 24;
pub const PERF_REG_ARM64_X25: u64 = 25;
pub const PERF_REG_ARM64_X26: u64 = 26;
pub const PERF_REG_ARM64_X27: u64 = 27;
pub const PERF_REG_ARM64_X28: u64 = 28;
pub const PERF_REG_ARM64_X29: u64 = 29;
pub const PERF_REG_ARM64_LR: u64 = 30;
pub const PERF_REG_ARM64_SP: u64 = 31;
pub const PERF_REG_ARM64_PC: u64 = 32;
pub const PERF_REG_ARM64_MAX: u64 = 33;

pub const PERF_SAMPLE_REGS_ABI_32: u64 = 1;
pub const PERF_SAMPLE_REGS_ABI_64: u64 = 2;

mod ioctl {
    use libc::c_ulong;

    #[cfg(not(any(
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    )))]
    mod arch {
        use libc::c_ulong;

        pub const IOC_SIZEBITS: c_ulong = 14;
        pub const IOC_DIRBITS: c_ulong = 2;
        pub const IOC_NONE: c_ulong = 0;
    }

    #[cfg(any(
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    ))]
    mod arch {
        use libc::c_ulong;

        pub const IOC_SIZEBITS: c_ulong = 13;
        pub const IOC_DIRBITS: c_ulong = 3;
        pub const IOC_NONE: c_ulong = 1;
    }

    pub use self::arch::*;

    pub const IOC_NRSHIFT: c_ulong = 0;
    pub const IOC_NRBITS: c_ulong = 8;
    pub const IOC_TYPEBITS: c_ulong = 8;
    pub const IOC_TYPESHIFT: c_ulong = IOC_NRSHIFT + IOC_NRBITS;
    pub const IOC_SIZESHIFT: c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
    pub const IOC_DIRSHIFT: c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
}

macro_rules! ioc {
    ($dir:expr, $kind:expr, $nr:expr, $size:expr) => {
        ($dir << ioctl::IOC_DIRSHIFT)
            | (($kind as c_ulong) << ioctl::IOC_TYPESHIFT)
            | ($nr << ioctl::IOC_NRSHIFT)
            | ($size << ioctl::IOC_SIZESHIFT)
    };
}

macro_rules! io {
    ($kind:expr, $nr:expr) => {
        ioc!(ioctl::IOC_NONE, $kind, $nr, 0)
    };
}

pub const PERF_EVENT_IOC_ENABLE: c_ulong = io!(b'$', 0);
pub const PERF_EVENT_IOC_DISABLE: c_ulong = io!(b'$', 1);

#[repr(C)]
pub struct PerfEventAttr {
    pub kind: u32,
    pub size: u32,
    pub config: u64,
    pub sample_period_or_freq: u64,
    pub sample_type: u64,
    pub read_format: u64,
    pub flags: u64,
    pub wakeup_events_or_watermark: u32,
    pub bp_type: u32,
    pub bp_addr_or_config: u64,
    pub bp_len_or_config: u64,
    pub branch_sample_type: u64,
    pub sample_regs_user: u64,
    pub sample_stack_user: u32,
    pub clock_id: i32,
    // Added in V4:
    // pub sample_regs_intr: u64,
    // Added in V5:
    // pub aux_watermark: u32,
    // pub reserved: u32
}

#[repr(C)]
pub struct PerfEventMmapPage {
    pub version: u32,
    pub compat_version: u32,
    pub lock: u32,
    pub index: u32,
    pub offset: i64,
    pub time_enabled: u64,
    pub time_running: u64,
    pub capabilities: u64,
    pub pmc_width: u16,
    pub time_shift: u16,
    pub time_mult: u32,
    pub time_offset: u64,
    pub time_zero: u64,
    pub size: u32,
    pub reserved: [u8; 118 * 8 + 4],
    pub data_head: u64,
    pub data_tail: u64,
    pub data_offset: u64,
    pub data_size: u64,
    pub aux_head: u64,
    pub aux_tail: u64,
    pub aux_offset: u64,
    pub aux_size: u64,
}

impl fmt::Debug for PerfEventMmapPage {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_map()
            .entry(&"version", &self.version)
            .entry(&"compat_version", &self.compat_version)
            .entry(&"lock", &self.lock)
            .entry(&"index", &self.index)
            .entry(&"offset", &self.offset)
            .entry(&"time_enabled", &self.time_enabled)
            .entry(&"time_running", &self.time_running)
            .entry(&"capabilities", &self.capabilities)
            .entry(&"pmc_width", &self.pmc_width)
            .entry(&"time_shift", &self.time_shift)
            .entry(&"time_mult", &self.time_mult)
            .entry(&"time_offset", &self.time_offset)
            .entry(&"time_zero", &self.time_zero)
            .entry(&"size", &self.size)
            .entry(&"data_head", &self.data_head)
            .entry(&"data_tail", &self.data_tail)
            .entry(&"data_offset", &self.data_offset)
            .entry(&"data_size", &self.data_size)
            .entry(&"aux_head", &self.aux_head)
            .entry(&"aux_tail", &self.aux_tail)
            .entry(&"aux_offset", &self.aux_offset)
            .entry(&"aux_size", &self.aux_size)
            .finish()
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct PerfEventHeader {
    pub kind: u32,
    pub misc: u16,
    pub size: u16,
}

pub fn sys_perf_event_open(
    attr: &PerfEventAttr,
    pid: pid_t,
    cpu: c_int,
    group_fd: c_int,
    flags: c_ulong,
) -> c_int {
    unsafe {
        syscall(
            SYS_perf_event_open,
            attr as *const _,
            pid,
            cpu,
            group_fd,
            flags,
        ) as c_int
    }
}
