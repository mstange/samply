use crate::thread_info::time_value;

use super::gecko_profile::ThreadBuilder;
use mach::mach_types::thread_act_t;
use std::mem;
use std::time::Instant;

use super::proc_maps::{get_backtrace, ForeignMemory};

use super::kernel_error::{self, IntoResult, KernelError};
use mach::port::mach_port_t;

use super::thread_act::thread_info;
use super::thread_info::{
    thread_basic_info_data_t, thread_extended_info_data_t, thread_identifier_info_data_t,
    thread_info_t, THREAD_BASIC_INFO, THREAD_BASIC_INFO_COUNT, THREAD_EXTENDED_INFO,
    THREAD_EXTENDED_INFO_COUNT, THREAD_IDENTIFIER_INFO, THREAD_IDENTIFIER_INFO_COUNT,
};

pub struct ThreadProfiler {
    process_start: Instant,
    thread_act: thread_act_t,
    _tid: u32,
    name: Option<String>,
    stack_scratch_space: Vec<u64>,
    thread_builder: ThreadBuilder,
    tick_count: usize,
    stack_memory: ForeignMemory,
    previous_sample_cpu_time: u64,
}

impl ThreadProfiler {
    pub fn new(
        task: mach_port_t,
        pid: u32,
        process_start: Instant,
        thread_act: thread_act_t,
        now: Instant,
        is_main: bool,
    ) -> kernel_error::Result<Option<Self>> {
        let (tid, is_libdispatch_thread) = match get_thread_id(thread_act) {
            Ok(info) => info,
            Err(KernelError::MachSendInvalidDest) => return Ok(None),
            Err(KernelError::InvalidArgument) => return Ok(None),
            Err(err) => return Err(err),
        };
        let thread_builder = ThreadBuilder::new(
            pid,
            tid,
            now.duration_since(process_start).as_secs_f64() * 1000.0,
            is_main,
            is_libdispatch_thread,
        );
        Ok(Some(ThreadProfiler {
            process_start,
            thread_act,
            _tid: tid,
            name: None,
            stack_scratch_space: Vec::new(),
            thread_builder,
            tick_count: 0,
            stack_memory: ForeignMemory::new(task),
            previous_sample_cpu_time: 0,
        }))
    }

    pub fn sample(&mut self, now: Instant) -> kernel_error::Result<bool> {
        let result = self.sample_impl(now);
        match result {
            Ok(()) => Ok(true),
            Err(KernelError::MachSendInvalidDest) => Ok(false),
            Err(KernelError::InvalidArgument) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn sample_impl(&mut self, now: Instant) -> kernel_error::Result<()> {
        self.tick_count += 1;

        if self.name.is_none() && self.tick_count % 10 == 1 {
            self.name = get_thread_name(self.thread_act)?;
            if let Some(name) = &self.name {
                self.thread_builder.set_name(name);
            }
        }

        let cpu_time = get_thread_cpu_time(self.thread_act)?;
        let cpu_time = cpu_time.0 + cpu_time.1;
        let cpu_delta = cpu_time.wrapping_sub(self.previous_sample_cpu_time);

        self.stack_scratch_space.clear();
        get_backtrace(
            &mut self.stack_memory,
            self.thread_act,
            &mut self.stack_scratch_space,
        )?;

        self.thread_builder.add_sample(
            now.duration_since(self.process_start).as_secs_f64() * 1000.0,
            &self.stack_scratch_space,
            cpu_delta,
        );

        self.previous_sample_cpu_time = cpu_time;

        Ok(())
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        self.thread_builder
            .notify_dead(end_time.duration_since(self.process_start).as_secs_f64() * 1000.0);
        self.stack_memory.clear();
    }

    pub fn into_profile_thread(self) -> ThreadBuilder {
        self.thread_builder
    }
}

/// Returns (tid, is_libdispatch_thread)
fn get_thread_id(thread_act: thread_act_t) -> kernel_error::Result<(u32, bool)> {
    let mut identifier_info_data: thread_identifier_info_data_t = unsafe { mem::zeroed() };
    let mut count = THREAD_IDENTIFIER_INFO_COUNT;
    unsafe {
        thread_info(
            thread_act,
            THREAD_IDENTIFIER_INFO,
            &mut identifier_info_data as *mut _ as thread_info_t,
            &mut count,
        )
    }
    .into_result()?;

    // This used to check dispatch_qaddr != 0, but it looks like this can happen
    // even for non-libdispatch threads, for example it happens for rust threads
    // such as the perfrecord sampler thread.
    let is_libdispatch_thread = false; // TODO

    Ok((identifier_info_data.thread_id as u32, is_libdispatch_thread))
}

fn get_thread_name(thread_act: thread_act_t) -> kernel_error::Result<Option<String>> {
    // Get the thread name.
    let mut extended_info_data: thread_extended_info_data_t = unsafe { mem::zeroed() };
    let mut count = THREAD_EXTENDED_INFO_COUNT;
    unsafe {
        thread_info(
            thread_act,
            THREAD_EXTENDED_INFO,
            &mut extended_info_data as *mut _ as thread_info_t,
            &mut count,
        )
    }
    .into_result()?;

    let name = unsafe { std::ffi::CStr::from_ptr(extended_info_data.pth_name.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

// (user time, system time) in integer microseconds
fn get_thread_cpu_time(thread_act: thread_act_t) -> kernel_error::Result<(u64, u64)> {
    // Get the thread name.
    let mut basic_info_data: thread_basic_info_data_t = unsafe { mem::zeroed() };
    let mut count = THREAD_BASIC_INFO_COUNT;
    unsafe {
        thread_info(
            thread_act,
            THREAD_BASIC_INFO,
            &mut basic_info_data as *mut _ as thread_info_t,
            &mut count,
        )
    }
    .into_result()?;

    Ok((
        time_value_to_microseconds(&basic_info_data.user_time),
        time_value_to_microseconds(&basic_info_data.system_time),
    ))
}

fn time_value_to_microseconds(tv: &time_value) -> u64 {
    tv.seconds as u64 * 1_000_000 + tv.microseconds as u64
}
