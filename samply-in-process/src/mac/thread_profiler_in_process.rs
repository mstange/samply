use std::mem;
use std::os::raw::c_void;

use framehop::FrameAddress;
use framehop_in_process::{
    InProcessUnwinder, InProcessUnwinderCache, InProcessUnwinderRegs, StackCollectionOutput,
};
use fxprof_processed_profile::{CpuDelta, Profile, StringHandle, ThreadHandle, Timestamp};
use libc::{pthread_from_mach_thread_np, pthread_get_stackaddr_np};
use mach2::mach_types::thread_act_t;
use mach2::port::mach_port_t;
use time::get_monotonic_timestamp;

use super::error::SamplingError;
use super::kernel_error::{self, IntoResult, KernelError};
use super::thread_act::thread_info; // https://github.com/JohnTitor/mach2/issues/31
use super::thread_info::{
    thread_basic_info_data_t, thread_extended_info_data_t, thread_identifier_info_data_t,
    thread_info_t, time_value, THREAD_BASIC_INFO, THREAD_BASIC_INFO_COUNT, THREAD_EXTENDED_INFO,
    THREAD_EXTENDED_INFO_COUNT, THREAD_IDENTIFIER_INFO, THREAD_IDENTIFIER_INFO_COUNT,
};
use crate::mac::proc_maps::{get_unwinding_registers, with_suspended_thread};
use crate::mac::time;
use crate::shared::recycling::ThreadRecycler;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{UnresolvedSamples, UnresolvedStacks};

pub struct StackwalkerRefInProcess<'a> {
    unwinder: &'a InProcessUnwinder,
    cache: &'a mut InProcessUnwinderCache,
}

impl<'a> StackwalkerRefInProcess<'a> {
    pub fn new(unwinder: &'a InProcessUnwinder, cache: &'a mut InProcessUnwinderCache) -> Self {
        Self { unwinder, cache }
    }
}

pub struct ThreadProfilerInProcess {
    thread_act: thread_act_t,
    name: Option<String>,
    pub(crate) tid: u32,
    pub(crate) profile_thread: ThreadHandle,
    thread_label: StringHandle,
    tick_count: usize,
    stack_base: *const c_void,
    previous_sample_cpu_time_us: u64,
    ignored_errors: Vec<SamplingError>,
}

impl ThreadProfilerInProcess {
    pub fn new(
        tid: u32,
        profile_thread: ThreadHandle,
        thread_label: StringHandle,
        thread_act: thread_act_t,
        name: Option<String>,
    ) -> Self {
        let stack_base = unsafe {
            let pthread = pthread_from_mach_thread_np(thread_act);
            pthread_get_stackaddr_np(pthread)
        };
        ThreadProfilerInProcess {
            thread_act,
            tid,
            name,
            profile_thread,
            thread_label,
            tick_count: 0,
            stack_base,
            previous_sample_cpu_time_us: 0,
            ignored_errors: Vec::new(),
        }
    }

    /// Called before every call to `sample`.
    pub fn check_thread_name(
        &mut self,
        profile: &mut Profile,
        thread_recycler: Option<&mut ThreadRecycler>,
    ) {
        if self.name.is_none() && self.tick_count % 10 == 0 {
            if let Ok(Some(name)) = get_thread_name(self.thread_act) {
                if let Some((thread_handle, thread_label)) =
                    thread_recycler.and_then(|tr| tr.recycle_by_name(&name))
                {
                    self.profile_thread = thread_handle;
                    self.thread_label = thread_label;
                } else {
                    profile.set_thread_name(self.profile_thread, &name);
                }
                self.name = Some(name);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        stackwalker: StackwalkerRefInProcess,
        now: Timestamp,
        now_mono: u64,
        stack_scratch_buffer: &mut Vec<FrameAddress>,
        output: &mut StackCollectionOutput,
        unresolved_stacks: &mut UnresolvedStacks,
        unresolved_samples: &mut UnresolvedSamples,
    ) -> Result<bool, SamplingError> {
        let result = self.sample_impl(
            stackwalker,
            now,
            now_mono,
            stack_scratch_buffer,
            output,
            unresolved_stacks,
            unresolved_samples,
        );
        match result {
            Ok(()) => Ok(true),
            Err(SamplingError::ThreadTerminated(_, _)) => Ok(false),
            Err(err @ SamplingError::Ignorable(_, _)) => {
                self.ignored_errors.push(err);
                if self.ignored_errors.len() >= 10 {
                    eprintln!(
                        "Treating thread \"{}\" [tid: {}] as terminated after 10 unknown errors:",
                        self.name.as_deref().unwrap_or("<unknown"),
                        self.tid
                    );
                    eprintln!("{:#?}", self.ignored_errors);
                    Ok(false)
                } else {
                    // Pretend that sampling worked and that the thread is still alive.
                    Ok(true)
                }
            }
            Err(err) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_impl(
        &mut self,
        stackwalker: StackwalkerRefInProcess,
        now: Timestamp,
        now_mono: u64,
        stack_scratch_buffer: &mut Vec<FrameAddress>,
        output: &mut StackCollectionOutput,
        unresolved_stacks: &mut UnresolvedStacks,
        unresolved_samples: &mut UnresolvedSamples,
    ) -> Result<(), SamplingError> {
        self.tick_count += 1;

        let cpu_time_us = get_thread_cpu_time_since_thread_start(self.thread_act)?;
        let cpu_time_us = cpu_time_us.0 + cpu_time_us.1;
        let cpu_delta_us = cpu_time_us - self.previous_sample_cpu_time_us;
        let cpu_delta = CpuDelta::from_micros(cpu_delta_us);

        if !cpu_delta.is_zero() || self.tick_count == 0 {
            get_backtrace_in_process(stackwalker, self.stack_base, self.thread_act, output)?;
            stack_scratch_buffer.clear();
            let pcs = &output.pc_buf[..*output.count];

            // make sure to use the time immediately after the stack is sampled so that any
            // jitdump records emitted in the interval between samply starting to sample
            // all tasks and actually stopping the thread are properly used
            let sample_time_mono = get_monotonic_timestamp();

            let frames = pcs.iter().enumerate().rev().map(|(i, pc)| {
                if i == 0 {
                    StackFrame::InstructionPointer(*pc as u64, StackMode::User)
                } else {
                    StackFrame::ReturnAddress(*pc as u64, StackMode::User)
                }
            });
            let stack = unresolved_stacks.convert(frames);
            unresolved_samples.add_sample(
                self.profile_thread,
                now,
                sample_time_mono,
                stack,
                cpu_delta,
                1,
                None,
            );
        } else {
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
            unresolved_samples.add_sample_same_stack_zero_cpu(
                self.profile_thread,
                now,
                now_mono,
                1,
                None,
            );
        }

        self.previous_sample_cpu_time_us = cpu_time_us;

        Ok(())
    }

    pub fn notify_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        profile.set_thread_end_time(self.profile_thread, end_time);
    }

    pub fn finish(self) -> (Option<String>, ThreadHandle, StringHandle) {
        (self.name, self.profile_thread, self.thread_label)
    }
}

/// `frames` must be empty initially.
///
/// On return, `frames` will have the stack frames from callee-most to root-most.
pub fn get_backtrace_in_process(
    stackwalker: StackwalkerRefInProcess,
    stack_base: *const c_void,
    thread_act: mach_port_t,
    output: &mut StackCollectionOutput,
) -> Result<(), SamplingError> {
    with_suspended_thread(thread_act, || {
        let (pc, regs) = get_unwinding_registers(thread_act).map_err(|err| match err {
            KernelError::InvalidArgument
            | KernelError::MachSendInvalidDest
            | KernelError::Terminated => {
                SamplingError::ThreadTerminated("thread_get_state in get_unwinding_registers", err)
            }
            err => SamplingError::Ignorable("thread_get_state in get_unwinding_registers", err),
        })?;
        let stack_mem = unsafe {
            core::slice::from_raw_parts(
                regs.sp() as *const usize,
                ((stack_base as usize)
                    .checked_sub(regs.sp() as usize)
                    .unwrap())
                    / size_of::<usize>(),
            )
        };
        let regs = InProcessUnwinderRegs {
            sp: regs.sp(),
            lr: regs.lr(),
            fp: regs.fp(),
            pc,
        };
        stackwalker
            .unwinder
            .walk_stack_and_collect(&regs, stack_mem, stackwalker.cache, output);
        Ok(())
    })
    .unwrap_or_else(|err| match err {
        KernelError::InvalidArgument
        | KernelError::MachSendInvalidDest
        | KernelError::Terminated => Err(SamplingError::ThreadTerminated(
            "thread_suspend in with_suspended_thread",
            err,
        )),
        err => Err(SamplingError::Ignorable(
            "thread_suspend in with_suspended_thread",
            err,
        )),
    })?;

    Ok(())
}
/// Returns (tid, is_libdispatch_thread)
pub fn get_thread_id(thread_act: thread_act_t) -> kernel_error::Result<(u32, bool)> {
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

pub fn get_thread_name(thread_act: thread_act_t) -> Result<Option<String>, SamplingError> {
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
            println!("thread_info in get_thread_name encountered unexpected error: {err:?}");
            SamplingError::Ignorable("thread_info in get_thread_name", err)
        }
    })?;

    let name = unsafe { std::ffi::CStr::from_ptr(extended_info_data.pth_name.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

// (user time, system time) in microseconds
fn get_thread_cpu_time_since_thread_start(
    thread_act: thread_act_t,
) -> Result<(u64, u64), SamplingError> {
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
        time_value_to_microseconds(&basic_info_data.user_time),
        time_value_to_microseconds(&basic_info_data.system_time),
    ))
}

fn time_value_to_microseconds(tv: &time_value) -> u64 {
    tv.seconds as u64 * 1_000_000 + tv.microseconds as u64
}
