use super::gecko_profile::ThreadBuilder;
use mach::mach_types::thread_act_t;
use std::io;
use std::mem;
use std::time::Instant;

use super::proc_maps::get_backtrace;

use mach::kern_return::KERN_SUCCESS;
use mach::port::mach_port_t;

use super::thread_act::thread_info;
use super::thread_info::{
    thread_extended_info_data_t, thread_identifier_info_data_t, thread_info_t,
    THREAD_EXTENDED_INFO, THREAD_EXTENDED_INFO_COUNT, THREAD_IDENTIFIER_INFO,
    THREAD_IDENTIFIER_INFO_COUNT,
};

pub struct ThreadProfiler {
    task: mach_port_t,
    process_start: Instant,
    thread_act: thread_act_t,
    _tid: u32,
    name: Option<String>,
    stack_scratch_space: Vec<u64>,
    thread_builder: ThreadBuilder,
    tick_count: usize,
}

impl ThreadProfiler {
    pub fn new(
        task: mach_port_t,
        pid: u32,
        process_start: Instant,
        thread_act: thread_act_t,
        now: Instant,
        is_main: bool,
    ) -> io::Result<Self> {
        let tid = get_thread_id(thread_act)? as u32;
        let mut thread_builder = ThreadBuilder::new(
            pid,
            tid,
            now.duration_since(process_start).as_secs_f64() * 1000.0,
        );
        if is_main {
            thread_builder.set_name("GeckoMain"); // https://github.com/firefox-devtools/profiler/issues/2508
        }
        Ok(ThreadProfiler {
            task,
            process_start,
            thread_act,
            _tid: tid,
            name: None,
            stack_scratch_space: Vec::new(),
            thread_builder,
            tick_count: 0,
        })
    }

    pub fn sample(&mut self, now: Instant) -> io::Result<()> {
        self.tick_count += 1;

        if self.name.is_none() && self.tick_count % 10 == 1 {
            self.name = get_thread_name(self.thread_act)?;
            if let Some(name) = &self.name {
                self.thread_builder.set_name(name);
            }
        }

        self.stack_scratch_space.clear();
        get_backtrace(self.task, self.thread_act, &mut self.stack_scratch_space)?;

        self.thread_builder.add_sample(
            now.duration_since(self.process_start).as_secs_f64() * 1000.0,
            &self.stack_scratch_space,
        );

        Ok(())
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        self.thread_builder
            .notify_dead(end_time.duration_since(self.process_start).as_secs_f64() * 1000.0);
    }

    pub fn into_profile_thread(self) -> ThreadBuilder {
        self.thread_builder
    }
}

fn get_thread_id(thread_act: thread_act_t) -> io::Result<u64> {
    let mut identifier_info_data: thread_identifier_info_data_t = unsafe { mem::zeroed() };
    let mut count = THREAD_IDENTIFIER_INFO_COUNT;
    let kret = unsafe {
        thread_info(
            thread_act,
            THREAD_IDENTIFIER_INFO,
            &mut identifier_info_data as *mut _ as thread_info_t,
            &mut count,
        )
    };
    if kret != KERN_SUCCESS {
        return Err(io::Error::last_os_error());
    }
    Ok(identifier_info_data.thread_id)
}

fn get_thread_name(thread_act: thread_act_t) -> io::Result<Option<String>> {
    // Get the thread name.
    let mut extended_info_data: thread_extended_info_data_t = unsafe { mem::zeroed() };
    let mut count = THREAD_EXTENDED_INFO_COUNT;
    let kret = unsafe {
        thread_info(
            thread_act,
            THREAD_EXTENDED_INFO,
            &mut extended_info_data as *mut _ as thread_info_t,
            &mut count,
        )
    };
    if kret != KERN_SUCCESS {
        return Err(io::Error::last_os_error());
    }

    let name = unsafe { std::ffi::CStr::from_ptr(extended_info_data.pth_name.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}
