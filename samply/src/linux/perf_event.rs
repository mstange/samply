use std::cell::RefCell;
use std::cmp::max;
use std::collections::BinaryHeap;
use std::io;
use std::mem;
use std::ops::Range;
use std::os::unix::io::RawFd;
use std::ptr;
use std::rc::Rc;
use std::slice;
use std::sync::atomic::fence;
use std::sync::atomic::Ordering;
use std::{cmp, fmt};

use libc::{self, c_void, pid_t};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_event_reader::{Endianness, RawData, RawEventRecord, RecordParseInfo, RecordType};

use super::sys::*;

#[derive(Debug)]
#[repr(C)]
struct PerfEventHeader {
    kind: u32,
    misc: u16,
    size: u16,
}

#[derive(Clone, Debug)]
enum SliceLocation {
    Single(Range<usize>),
    Split(Range<usize>, Range<usize>),
}

impl SliceLocation {
    #[inline]
    fn get<'a>(&self, buffer: &'a [u8]) -> RawData<'a> {
        match *self {
            SliceLocation::Single(ref range) => RawData::Single(&buffer[range.clone()]),
            SliceLocation::Split(ref left, ref right) => {
                RawData::Split(&buffer[left.clone()], &buffer[right.clone()])
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RawRecordLocation {
    kind: u32,
    misc: u16,
    data_location: SliceLocation,
}

impl RawRecordLocation {
    #[inline]
    fn get<'a>(&self, buffer: &'a [u8], parse_info: RecordParseInfo) -> RawEventRecord<'a> {
        RawEventRecord {
            record_type: RecordType(self.kind),
            misc: self.misc,
            data: self.data_location.get(buffer),
            parse_info,
        }
    }
}

unsafe fn read_head(pointer: *const u8) -> u64 {
    let page = &*(pointer as *const PerfEventMmapPage);
    let head = ptr::read_volatile(&page.data_head);
    fence(Ordering::Acquire);
    head
}

unsafe fn read_tail(pointer: *const u8) -> u64 {
    let page = &*(pointer as *const PerfEventMmapPage);
    // No memory fence required because we're just reading a value previously
    // written by us.
    ptr::read_volatile(&page.data_tail)
}

unsafe fn write_tail(pointer: *mut u8, value: u64) {
    let page = &mut *(pointer as *mut PerfEventMmapPage);
    fence(Ordering::AcqRel);
    ptr::write_volatile(&mut page.data_tail, value);
}

#[derive(Debug)]
pub struct Perf {
    event_ref_state: Rc<RefCell<EventRefState>>,
    buffer: *mut u8,
    size: u64,
    fd: RawFd,
    position: u64,
    parse_info: RecordParseInfo,
}

impl Drop for Perf {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[inline]
unsafe fn get_buffer<'a>(buffer: *const u8, size: u64) -> &'a [u8] {
    slice::from_raw_parts(buffer.offset(4096), size as usize)
}

fn next_raw_event(
    buffer: *const u8,
    size: u64,
    position_cell: &mut u64,
) -> Option<RawRecordLocation> {
    let head = unsafe { read_head(buffer) };
    if head == *position_cell {
        return None;
    }

    let buffer = unsafe { get_buffer(buffer, size) };
    let position = *position_cell;
    let relative_position = position % size;
    let event_position = relative_position as usize;
    let event_data_position =
        (relative_position + mem::size_of::<PerfEventHeader>() as u64) as usize;
    let event_header = unsafe {
        &*(&buffer[event_position..event_data_position] as *const _ as *const PerfEventHeader)
    };
    let next_event_position = event_position + event_header.size as usize;

    let data_location = if next_event_position > size as usize {
        let first = event_data_position..buffer.len();
        let second = 0..next_event_position % size as usize;
        SliceLocation::Split(first, second)
    } else {
        SliceLocation::Single(event_data_position..next_event_position)
    };

    let raw_event_location = RawRecordLocation {
        kind: event_header.kind,
        misc: event_header.misc,
        data_location,
    };

    // trace!("Parsed raw event: {:?}", raw_event_location);

    let next_position = position + event_header.size as u64;
    *position_cell = next_position;

    Some(raw_event_location)
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum EventSource {
    HwCpuCycles,
    SwCpuClock,
}

#[derive(Clone, Debug)]
pub struct PerfBuilder {
    pid: u32,
    cpu: Option<u32>,
    frequency: u64,
    stack_size: u32,
    reg_mask: u64,
    event_source: EventSource,
    inherit: bool,
    start_disabled: bool,
    enable_on_exec: bool,
    exclude_kernel: bool,
    gather_context_switches: bool,
}

impl PerfBuilder {
    pub fn pid(mut self, pid: u32) -> Self {
        self.pid = pid;
        self
    }

    pub fn only_cpu(mut self, cpu: u32) -> Self {
        self.cpu = Some(cpu);
        self
    }

    pub fn any_cpu(mut self) -> Self {
        self.cpu = None;
        self
    }

    pub fn frequency(mut self, frequency: u64) -> Self {
        self.frequency = frequency;
        self
    }

    pub fn sample_user_stack(mut self, stack_size: u32) -> Self {
        self.stack_size = stack_size;
        self
    }

    pub fn sample_user_regs(mut self, reg_mask: u64) -> Self {
        self.reg_mask = reg_mask;
        self
    }

    /// Turns on the kernel measurements. This requires the `/proc/sys/kernel/perf_event_paranoid` to be less than `2`.
    pub fn sample_kernel(mut self) -> Self {
        self.exclude_kernel = false;
        self
    }

    pub fn event_source(mut self, event_source: EventSource) -> Self {
        self.event_source = event_source;
        self
    }

    pub fn inherit_to_children(mut self) -> Self {
        self.inherit = true;
        self
    }

    pub fn start_disabled(mut self) -> Self {
        self.start_disabled = true;
        self
    }

    pub fn enable_on_exec(mut self) -> Self {
        self.enable_on_exec = true;
        self
    }

    pub fn gather_context_switches(mut self) -> Self {
        self.gather_context_switches = true;
        self
    }

    pub fn open(self) -> io::Result<Perf> {
        let pid = self.pid;
        let cpu = self.cpu.map(|cpu| cpu as i32).unwrap_or(-1);
        let frequency = self.frequency;
        let stack_size = self.stack_size;
        let reg_mask = self.reg_mask;
        let event_source = self.event_source;
        let inherit = self.inherit;
        let start_disabled = self.start_disabled;
        let exclude_kernel = self.exclude_kernel;
        let gather_context_switches = self.gather_context_switches;

        // debug!(
        //     "Opening perf events; pid={}, cpu={}, frequency={}, stack_size={}, reg_mask=0x{:016X}, event_source={:?}, inherit={}, start_disabled={}...",
        //     pid,
        //     cpu,
        //     frequency,
        //     stack_size,
        //     reg_mask,
        //     event_source,
        //     inherit,
        //     start_disabled
        // );

        let max_sample_rate = Perf::max_sample_rate();
        if let Some(max_sample_rate) = max_sample_rate {
            // debug!("Maximum sample rate: {}", max_sample_rate);
            if frequency > max_sample_rate {
                let message = format!( "frequency can be at most {max_sample_rate} as configured in /proc/sys/kernel/perf_event_max_sample_rate" );
                return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
            }
        }

        if stack_size > 63 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sample_user_stack can be at most 63kb",
            ));
        }

        // See `perf_mmap` in the Linux kernel.
        if cpu == -1 && inherit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "you can't inherit to children and run on all cpus at the same time",
            ));
        }

        assert_eq!(mem::size_of::<PerfEventMmapPage>(), 1088);

        if cfg!(target_arch = "x86_64") {
            assert_eq!(PERF_EVENT_IOC_ENABLE, 9216);
        } else if cfg!(target_arch = "mips64") {
            assert_eq!(PERF_EVENT_IOC_ENABLE, 536880128);
        }

        let mut attr: PerfEventAttr = unsafe { mem::zeroed() };
        attr.size = mem::size_of::<PerfEventAttr>() as u32;

        match event_source {
            EventSource::HwCpuCycles => {
                attr.kind = PERF_TYPE_HARDWARE;
                attr.config = PERF_COUNT_HW_CPU_CYCLES;
            }
            EventSource::SwCpuClock => {
                attr.kind = PERF_TYPE_SOFTWARE;
                attr.config = PERF_COUNT_SW_CPU_CLOCK;
            }
        }

        attr.sample_type = PERF_SAMPLE_IP
            | PERF_SAMPLE_TID
            | PERF_SAMPLE_TIME
            | PERF_SAMPLE_CPU
            | PERF_SAMPLE_PERIOD;

        if reg_mask != 0 {
            attr.sample_type |= PERF_SAMPLE_REGS_USER;
        }

        if stack_size != 0 {
            attr.sample_type |= PERF_SAMPLE_STACK_USER;
        }

        attr.sample_regs_user = reg_mask;
        attr.sample_stack_user = stack_size;
        attr.sample_period_or_freq = frequency;
        attr.clock_id = libc::CLOCK_MONOTONIC;

        attr.flags = PERF_ATTR_FLAG_DISABLED
            | PERF_ATTR_FLAG_MMAP
            | PERF_ATTR_FLAG_MMAP2
            | PERF_ATTR_FLAG_MMAP_DATA
            | PERF_ATTR_FLAG_COMM
            | PERF_ATTR_FLAG_FREQ
            | PERF_ATTR_FLAG_TASK
            | PERF_ATTR_FLAG_SAMPLE_ID_ALL
            | PERF_ATTR_FLAG_USE_CLOCKID;

        if self.enable_on_exec {
            attr.flags |= PERF_ATTR_FLAG_ENABLE_ON_EXEC;
        }

        if exclude_kernel {
            attr.flags |= PERF_ATTR_FLAG_EXCLUDE_KERNEL;
        }

        if inherit {
            attr.flags |= PERF_ATTR_FLAG_INHERIT;
        }

        if gather_context_switches {
            attr.flags |= PERF_ATTR_FLAG_CONTEX_SWITCH;
        }

        let fd = sys_perf_event_open(&attr, pid as pid_t, cpu as _, -1, PERF_FLAG_FD_CLOEXEC);
        if fd < 0 {
            let err = io::Error::from_raw_os_error(-fd);
            // eprintln!(
            //     "The perf_event_open syscall failed for PID {}: {}",
            //     pid, err
            // );
            if let Some(errcode) = err.raw_os_error() {
                if errcode == libc::EINVAL {
                    // info!("Your profiling frequency might be too high; try lowering it");
                }
            }

            return Err(err);
        }

        const STACK_COUNT_PER_BUFFER: u32 = 32;
        let required_space = max(stack_size, 4096) * STACK_COUNT_PER_BUFFER;
        let page_size = 4096;
        let n = (1..26)
            .find(|n| (1_u32 << n) * 4096_u32 >= required_space)
            .expect("cannot find appropriate page count for given stack size");
        let page_count: u32 = max(1 << n, 16);
        // debug!(
        //     "Allocating {} + 1 pages for the ring buffer for PID {} on CPU {}",
        //     page_count, pid, cpu
        // );

        let full_size = (page_size * (page_count + 1)) as usize;

        let buffer;
        unsafe {
            buffer = libc::mmap(
                ptr::null_mut(),
                full_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if buffer == libc::MAP_FAILED {
                libc::close(fd);
                return Err(io::Error::new(io::ErrorKind::Other, "mmap failed"));
            }
        }

        let buffer = buffer as *mut u8;
        let size = (page_size * page_count) as u64;

        let attr_bytes_ptr = &attr as *const PerfEventAttr as *const u8;
        let attr_bytes_len = mem::size_of::<PerfEventAttr>();
        let attr_bytes = unsafe { slice::from_raw_parts(attr_bytes_ptr, attr_bytes_len) };
        let (attr2, _size) =
            linux_perf_event_reader::PerfEventAttr::parse::<_, byteorder::NativeEndian>(attr_bytes)
                .unwrap();
        let parse_info = RecordParseInfo::new(&attr2, Endianness::NATIVE);

        // debug!("Perf events open with fd={}", fd);
        let mut perf = Perf {
            event_ref_state: Rc::new(RefCell::new(EventRefState::new(buffer, size))),
            buffer,
            size,
            fd,
            position: 0,
            parse_info,
        };

        if !start_disabled {
            perf.enable();
        }

        Ok(perf)
    }
}

impl Perf {
    pub fn max_sample_rate() -> Option<u64> {
        let data = std::fs::read_to_string("/proc/sys/kernel/perf_event_max_sample_rate").ok()?;
        data.trim().parse::<u64>().ok()
    }

    pub fn build() -> PerfBuilder {
        PerfBuilder {
            pid: 0,
            cpu: None,
            frequency: 0,
            stack_size: 0,
            reg_mask: 0,
            event_source: EventSource::SwCpuClock,
            inherit: false,
            start_disabled: false,
            enable_on_exec: false,
            exclude_kernel: true,
            gather_context_switches: false,
        }
    }

    pub fn enable(&mut self) {
        let result = unsafe { libc::ioctl(self.fd, PERF_EVENT_IOC_ENABLE as _) };

        assert!(result != -1);
    }

    #[inline]
    pub fn are_events_pending(&self) -> bool {
        let head = unsafe { read_head(self.buffer) };
        head != self.position
    }

    #[inline]
    pub fn fd(&self) -> RawFd {
        self.fd
    }

    #[inline]
    pub fn iter(&mut self) -> EventIter {
        EventIter::new(self)
    }
}

#[derive(Debug)]
struct EventRefState {
    buffer: *mut u8,
    size: u64,
    pending_commits: BinaryHeap<cmp::Reverse<(u64, u64)>>,
}

impl EventRefState {
    fn new(buffer: *mut u8, size: u64) -> Self {
        EventRefState {
            buffer,
            size,
            pending_commits: BinaryHeap::new(),
        }
    }

    /// Mark the read of [from, to) as complete.
    /// If reads are completed in-order, then this will advance the tail pointer to `to` immediately.
    /// Otherwise, it will remain in the "pending commit" queue, and committed once all previous
    /// reads are also committed.
    fn try_commit(&mut self, from: u64, to: u64) {
        self.pending_commits.push(cmp::Reverse((from, to)));

        let mut position = unsafe { read_tail(self.buffer) };
        while let Some(&cmp::Reverse((from, to))) = self.pending_commits.peek() {
            if from == position {
                unsafe {
                    write_tail(self.buffer, to);
                }
                position = to;
                self.pending_commits.pop();
            } else {
                break;
            }
        }
    }
}

impl Drop for EventRefState {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.buffer as *mut c_void, (self.size + 4096) as _);
        }
    }
}

/// Handle to a single event in the perf ring buffer.
///
/// On Drop, the event will be "consumed" and the read pointer will be advanced.
///
/// If events are dropped out of order, then it will be added to a list of pending commits and
/// committed when all prior events are also dropped. For this reason, events should be dropped
/// in-order to achieve the lowest overhead.
#[derive(Clone)]
pub struct EventRef {
    buffer: *mut u8,
    buffer_size: usize,
    state: Rc<RefCell<EventRefState>>,
    event_location: RawRecordLocation,
    prev_position: u64,
    position: u64,
    parse_info: RecordParseInfo,
}

impl fmt::Debug for EventRef {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_map()
            .entry(&"location", &self.event_location)
            .entry(&"prev_position", &self.prev_position)
            .entry(&"position", &self.position)
            .finish()
    }
}

impl Drop for EventRef {
    #[inline]
    fn drop(&mut self) {
        self.state
            .borrow_mut()
            .try_commit(self.prev_position, self.position);
    }
}

impl EventRef {
    pub fn get(&self) -> RawEventRecord<'_> {
        let buffer = unsafe { slice::from_raw_parts(self.buffer.offset(4096), self.buffer_size) };

        self.event_location.get(buffer, self.parse_info)
    }
}

pub struct EventIter<'a> {
    perf: &'a mut Perf,
}

impl<'a> EventIter<'a> {
    #[inline]
    fn new(perf: &'a mut Perf) -> Self {
        EventIter { perf }
    }
}

impl<'a> Iterator for EventIter<'a> {
    type Item = EventRef;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let perf = &mut self.perf;
        let prev_position = perf.position;
        let event_location = next_raw_event(perf.buffer, perf.size, &mut perf.position)?;
        Some(EventRef {
            buffer: perf.buffer,
            buffer_size: perf.size as usize,
            state: perf.event_ref_state.clone(),
            event_location,
            prev_position,
            position: perf.position,
            parse_info: self.perf.parse_info,
        })
    }
}
