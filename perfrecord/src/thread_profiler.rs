use crate::error::SamplingError;
use crate::kernel_error::{self, IntoResult, KernelError};
use crate::proc_maps::{get_backtrace, ForeignMemory, StackwalkerRef};
use crate::thread_act::thread_info;
use crate::thread_info::time_value;
use crate::thread_info::{
    thread_basic_info_data_t, thread_extended_info_data_t, thread_identifier_info_data_t,
    thread_info_t, THREAD_BASIC_INFO, THREAD_BASIC_INFO_COUNT, THREAD_EXTENDED_INFO,
    THREAD_EXTENDED_INFO_COUNT, THREAD_IDENTIFIER_INFO, THREAD_IDENTIFIER_INFO_COUNT,
};
use gecko_profile::{Frame, ThreadBuilder};
use mach::mach_types::thread_act_t;
use mach::port::mach_port_t;
use std::mem;
use std::time::{Duration, Instant};

pub struct ThreadProfiler {
    thread_act: thread_act_t,
    _tid: u32,
    name: Option<String>,
    stack_scratch_space: Vec<u64>,
    thread_builder: ThreadBuilder,
    tick_count: usize,
    stack_memory: ForeignMemory,
    previous_sample_cpu_time: Duration,
    previous_stack: Option<Option<usize>>,
    ignored_errors: Vec<SamplingError>,
}

impl ThreadProfiler {
    pub fn new(
        task: mach_port_t,
        pid: u32,
        thread_act: thread_act_t,
        now: Instant,
        is_main: bool,
    ) -> Option<Self> {
        let (tid, is_libdispatch_thread) = get_thread_id(thread_act).ok()?;
        let thread_builder = ThreadBuilder::new(pid, tid, now, is_main, is_libdispatch_thread);
        Some(ThreadProfiler {
            thread_act,
            _tid: tid,
            name: None,
            stack_scratch_space: Vec::new(),
            thread_builder,
            tick_count: 0,
            stack_memory: ForeignMemory::new(task),
            previous_sample_cpu_time: Duration::ZERO,
            previous_stack: None,
            ignored_errors: Vec::new(),
        })
    }

    pub fn sample(
        &mut self,
        stackwalker: StackwalkerRef,
        now: Instant,
    ) -> Result<bool, SamplingError> {
        let result = self.sample_impl(stackwalker, now);
        match result {
            Ok(()) => Ok(true),
            Err(SamplingError::ThreadTerminated(_, _)) => Ok(false),
            Err(err @ SamplingError::Ignorable(_, _)) => {
                self.ignored_errors.push(err);
                if self.ignored_errors.len() >= 10 {
                    println!(
                        "Treating thread \"{}\" [tid: {}] as terminated after 10 unknown errors:",
                        self.name.as_deref().unwrap_or("<unknown"),
                        self._tid
                    );
                    println!("{:#?}", self.ignored_errors);
                    Ok(false)
                } else {
                    // Pretend that sampling worked and that the thread is still alive.
                    Ok(true)
                }
            }
            Err(err) => Err(err),
        }
    }

    fn sample_impl(
        &mut self,
        stackwalker: StackwalkerRef,
        now: Instant,
    ) -> Result<(), SamplingError> {
        self.tick_count += 1;

        if self.name.is_none() && self.tick_count % 10 == 1 {
            self.name = get_thread_name(self.thread_act)?;
            if let Some(name) = &self.name {
                self.thread_builder.set_name(name);
            }
        }

        let cpu_time = get_thread_cpu_time_since_thread_start(self.thread_act)?;
        let cpu_time = cpu_time.0 + cpu_time.1;
        let cpu_delta = cpu_time - self.previous_sample_cpu_time;

        if !cpu_delta.is_zero() || self.previous_stack.is_none() {
            self.stack_scratch_space.clear();
            get_backtrace(
                stackwalker,
                &mut self.stack_memory,
                self.thread_act,
                &mut self.stack_scratch_space,
            )?;

            let frames = self
                .stack_scratch_space
                .iter()
                .map(|address| Frame::Address(*address));

            let stack = self.thread_builder.add_sample(now, frames, cpu_delta);
            self.previous_stack = Some(stack);
        } else if let Some(previous_stack) = self.previous_stack {
            // No CPU time elapsed since just before the last time we grabbed a stack.
            // Assume that the thread has done literally zero work and could not have changed
            // its stack. This considerably reduces the overhead from sampling idle threads.
            //
            // More specifically, we hit this path after the following order of events
            //  - sample n-1:
            //     - query cpu time, call it A
            //     - pause the thread
            //     - walk the stack
            //     - resume the thread
            //  - sleep till next sample
            //  - sample n:
            //     - query cpu time, notice it is still the same as A
            //     - add_sample_same_stack with stack from previous sample
            //
            self.thread_builder
                .add_sample_same_stack(now, previous_stack, cpu_delta);
        }

        self.previous_sample_cpu_time = cpu_time;

        Ok(())
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        self.thread_builder.notify_dead(end_time);
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

fn get_thread_name(thread_act: thread_act_t) -> Result<Option<String>, SamplingError> {
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
    .into_result()
    .map_err(|err| match err {
        KernelError::InvalidArgument
        | KernelError::MachSendInvalidDest
        | KernelError::Terminated => {
            SamplingError::ThreadTerminated("thread_info in get_thread_name", err)
        }
        err => {
            println!(
                "thread_info in get_thread_name encountered unexpected error: {:?}",
                err
            );
            SamplingError::Ignorable("thread_info in get_thread_name", err)
        }
    })?;

    let name = unsafe { std::ffi::CStr::from_ptr(extended_info_data.pth_name.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

// (user time, system time)
fn get_thread_cpu_time_since_thread_start(
    thread_act: thread_act_t,
) -> Result<(Duration, Duration), SamplingError> {
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
    .into_result()
    .map_err(|err| match err {
        KernelError::InvalidArgument
        | KernelError::MachSendInvalidDest
        | KernelError::Terminated => SamplingError::ThreadTerminated(
            "thread_info in get_thread_cpu_time_since_thread_start",
            err,
        ),
        err => {
            SamplingError::Ignorable("thread_info in get_thread_cpu_time_since_thread_start", err)
        }
    })?;

    Ok((
        time_value_to_duration(&basic_info_data.user_time),
        time_value_to_duration(&basic_info_data.system_time),
    ))
}

fn time_value_to_duration(tv: &time_value) -> Duration {
    Duration::from_secs(tv.seconds as u64) + Duration::from_micros(tv.microseconds as u64)
}
