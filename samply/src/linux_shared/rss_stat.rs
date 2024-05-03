use std::fmt::Debug;

use byteorder::ByteOrder;
use linux_perf_data::{linux_perf_event_reader, Endianness};
use linux_perf_event_reader::RawData;

/// Resident file mapping pages
#[allow(unused)]
pub const MM_FILEPAGES: i32 = 0;

/// Resident anonymous pages
#[allow(unused)]
pub const MM_ANONPAGES: i32 = 1;

/// Anonymous swap entries
#[allow(unused)]
pub const MM_SWAPENTS: i32 = 2;

/// Resident shared memory pages
#[allow(unused)]
pub const MM_SHMEMPAGES: i32 = 3;

/// ```
/// # cat /sys/kernel/debug/tracing/events/kmem/rss_stat/format
/// name: rss_stat
/// ID: 537
/// format:
///         field:unsigned short common_type;       offset:0;       size:2; signed:0;
///         field:unsigned char common_flags;       offset:2;       size:1; signed:0;
///         field:unsigned char common_preempt_count;       offset:3;       size:1; signed:0;
///         field:int common_pid;   offset:4;       size:4; signed:1;
///
///         field:unsigned int mm_id;       offset:8;       size:4; signed:0;
///         field:unsigned int curr;        offset:12;      size:4; signed:0;
///         field:int member;       offset:16;      size:4; signed:1;
///         field:long size;        offset:24;      size:8; signed:1;
///
/// print fmt: "mm_id=%u curr=%d type=%s size=%ldB", REC->mm_id, REC->curr, __print_symbolic(REC->member, { 0, "MM_FILEPAGES" }, { 1, "MM_ANONPAGES" }, { 2, "MM_SWAPENTS" }, { 3, "MM_SHMEMPAGES" }), REC->size
/// ```
#[repr(C)]
#[derive(Debug)]
pub struct RssStat {
    pub common_type: u16,
    pub common_flags: u8,
    pub common_preempt_count: u8,
    pub common_pid: i32,
    pub mm_id: u32,
    pub curr: u32,
    pub member: i32,
    pub size: i64,
}

impl RssStat {
    pub fn parse(data: RawData, endian: Endianness) -> Result<Self, std::io::Error> {
        match endian {
            Endianness::LittleEndian => Self::parse_impl::<byteorder::LittleEndian>(data),
            Endianness::BigEndian => Self::parse_impl::<byteorder::BigEndian>(data),
        }
    }

    pub fn parse_impl<O: ByteOrder>(mut data: RawData) -> Result<Self, std::io::Error> {
        let common_type = data.read_u16::<O>()?;
        let common_flags = data.read_u8()?;
        let common_preempt_count = data.read_u8()?;
        let common_pid = data.read_i32::<O>()?;
        let mm_id = data.read_u32::<O>()?;
        let curr = data.read_u32::<O>()?;
        let member = data.read_i32::<O>()?;
        let _padding = data.read_u32::<O>()?;
        let size = data.read_u64::<O>()? as i64;
        Ok(RssStat {
            common_type,
            common_flags,
            common_preempt_count,
            common_pid,
            mm_id,
            curr,
            member,
            size,
        })
    }
}
