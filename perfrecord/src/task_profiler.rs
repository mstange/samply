use super::proc_maps::{get_dyld_info, DyldInfo};
use super::thread_profiler::ThreadProfiler;
use mach;
use mach::kern_return::KERN_SUCCESS;
use mach::mach_types::thread_act_port_array_t;
use mach::mach_types::thread_act_t;
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::task::task_threads;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::time::Instant;

use super::gecko_profile::ProfileBuilder;

pub struct TaskProfiler {
    task: mach_port_t,
    start_time: Instant,
    _end_time: Option<Instant>,
    live_threads: HashMap<thread_act_t, ThreadProfiler>,
    dead_threads: Vec<ThreadProfiler>,
    libs: Vec<DyldInfo>,
}

impl TaskProfiler {
    pub fn new(task: mach_port_t, now: Instant) -> io::Result<Self> {
        let thread_acts = get_thread_list(task)?;
        let mut live_threads = HashMap::new();
        for thread_act in thread_acts {
            live_threads.insert(thread_act, ThreadProfiler::new(task, now, thread_act, now)?);
        }
        Ok(TaskProfiler {
            task,
            start_time: now,
            _end_time: None,
            live_threads,
            dead_threads: Vec::new(),
            libs: get_dyld_info(task)?,
        })
    }

    pub fn sample(&mut self, now: Instant) -> io::Result<()> {
        let mut previously_live_threads: HashSet<_> =
            self.live_threads.iter().map(|(t, _)| *t).collect();
        let thread_acts = get_thread_list(self.task)?;
        for thread_act in thread_acts {
            if self.live_threads.get(&thread_act).is_none() {
                self.live_threads.insert(
                    thread_act,
                    ThreadProfiler::new(self.task, self.start_time, thread_act, now)?,
                );
            }
            previously_live_threads.remove(&thread_act);
        }
        for dead_thread_act in previously_live_threads {
            let mut dead_thread = self.live_threads.remove(&dead_thread_act).unwrap();
            dead_thread.notify_dead(now);
            self.dead_threads.push(dead_thread);
        }
        for (_, thread) in &mut self.live_threads {
            thread.sample(now)?;
        }
        Ok(())
    }

    pub fn into_profile(self) -> ProfileBuilder {
        let mut profile_builder = ProfileBuilder::new(self.start_time);
        let all_threads = self
            .live_threads
            .into_iter()
            .map(|(_, t)| t)
            .chain(self.dead_threads.into_iter())
            .map(|t| t.into_profile_thread());
        for thread in all_threads {
            profile_builder.add_thread(thread);
        }

        for DyldInfo {
            file,
            uuid,
            address,
            vmsize,
        } in self.libs
        {
            let uuid = match uuid {
                Some(uuid) => uuid,
                None => continue,
            };
            let name = Path::new(&file).file_name().unwrap().to_str().unwrap();
            let address_range = address..(address + vmsize);
            profile_builder.add_lib(&name, &file, &uuid, &address_range);
        }

        profile_builder
    }
}

fn get_thread_list(task: mach_port_t) -> io::Result<Vec<thread_act_t>> {
    let mut thread_list: thread_act_port_array_t = std::ptr::null_mut();
    let mut thread_count: mach_msg_type_number_t = Default::default();
    let kret = unsafe { task_threads(task, &mut thread_list, &mut thread_count) };
    if kret != KERN_SUCCESS {
        return Err(io::Error::last_os_error());
    }

    let thread_acts = unsafe { std::slice::from_raw_parts(thread_list, thread_count as usize) };
    // leak thread_list or what?
    Ok(thread_acts.to_owned())
}
