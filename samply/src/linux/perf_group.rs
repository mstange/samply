use std::cell::Cell;
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::os::unix::io::RawFd;
use std::{fs, io, vec};

use super::perf_event::{EventRef, EventSource, Perf};

struct StoppedProcess(u32);

impl StoppedProcess {
    fn new(pid: u32) -> Result<Self, io::Error> {
        // debug!("Stopping process with PID {}...", pid);
        let ok = unsafe { libc::kill(pid as _, libc::SIGSTOP) };
        if ok < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(StoppedProcess(pid))
    }
}

impl Drop for StoppedProcess {
    fn drop(&mut self) {
        // debug!("Resuming process with PID {}...", self.0);
        unsafe {
            libc::kill(self.0 as _, libc::SIGCONT);
        }
    }
}

struct Member {
    perf: Perf,
    is_closed: Cell<bool>,
}

impl Member {
    fn new(perf: Perf) -> Self {
        Member {
            perf,
            is_closed: Cell::new(false),
        }
    }
}

impl Deref for Member {
    type Target = Perf;
    fn deref(&self) -> &Self::Target {
        &self.perf
    }
}

impl DerefMut for Member {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.perf
    }
}

pub struct PerfGroup {
    event_buffer: Vec<EventRef>,
    members: BTreeMap<RawFd, Member>,
    poll_fds: Vec<libc::pollfd>,
    frequency: u32,
    stack_size: u32,
    regs_mask: u64,
    event_source: EventSource,
    stopped_processes: Vec<StoppedProcess>,
}

fn poll_events<'a, I>(poll_fds: &mut Vec<libc::pollfd>, iter: I)
where
    I: IntoIterator<Item = &'a Member>,
    <I as IntoIterator>::IntoIter: Clone,
{
    let iter = iter.into_iter();

    poll_fds.clear();
    poll_fds.extend(iter.clone().map(|member| libc::pollfd {
        fd: member.fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    }));

    let ok = unsafe { libc::poll(poll_fds.as_ptr() as *mut _, poll_fds.len() as _, 1000) };
    if ok == -1 {
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            panic!("poll failed: {}", err);
        }
    }

    for (member, poll_fd) in iter.zip(poll_fds.iter()) {
        member.is_closed.set(poll_fd.revents & libc::POLLHUP != 0);
    }
}

fn get_threads(pid: u32) -> Result<Vec<u32>, io::Error> {
    let entries = fs::read_dir(format!("/proc/{pid}/task"))?;
    let tids = entries
        .flatten()
        .filter_map(|entry| {
            let tid: u32 = entry.file_name().to_string_lossy().parse().unwrap();
            if tid != pid {
                Some(tid)
            } else {
                None
            }
        })
        .collect();
    Ok(tids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    AttachWithEnableOnExec,
    StopAttachEnableResume,
}

impl PerfGroup {
    pub fn new(frequency: u32, stack_size: u32, regs_mask: u64, event_source: EventSource) -> Self {
        PerfGroup {
            event_buffer: Vec::new(),
            members: Default::default(),
            poll_fds: Vec::new(),
            frequency,
            stack_size,
            event_source,
            regs_mask,
            stopped_processes: Vec::new(),
        }
    }

    pub fn open(
        pid: u32,
        frequency: u32,
        stack_size: u32,
        event_source: EventSource,
        regs_mask: u64,
        attach_mode: AttachMode,
    ) -> Result<Self, io::Error> {
        let mut group = PerfGroup::new(frequency, stack_size, regs_mask, event_source);
        group.open_process(pid, attach_mode)?;
        Ok(group)
    }

    pub fn open_process(&mut self, pid: u32, attach_mode: AttachMode) -> Result<(), io::Error> {
        if attach_mode == AttachMode::StopAttachEnableResume {
            self.stopped_processes.push(StoppedProcess::new(pid)?);
        }
        let mut perf_events = Vec::new();
        let threads = get_threads(pid)?;

        let cpu_count = num_cpus::get();
        for cpu in 0..cpu_count as u32 {
            let mut builder = Perf::build()
                .pid(pid)
                .only_cpu(cpu as _)
                .frequency(self.frequency as u64)
                .sample_user_stack(self.stack_size)
                .sample_user_regs(self.regs_mask)
                .sample_kernel()
                .gather_context_switches()
                .event_source(self.event_source)
                .inherit_to_children()
                .start_disabled();

            if attach_mode == AttachMode::AttachWithEnableOnExec {
                builder = builder.enable_on_exec();
            }

            let perf = builder.open()?;

            perf_events.push((Some(cpu), perf));
        }

        if cpu_count * (threads.len() + 1) >= 1000 {
            for &tid in &threads {
                let mut builder = Perf::build()
                    .pid(tid)
                    .any_cpu()
                    .frequency(self.frequency as u64)
                    .sample_user_stack(self.stack_size)
                    .sample_user_regs(self.regs_mask)
                    .sample_kernel()
                    .event_source(self.event_source)
                    .start_disabled();
                if attach_mode == AttachMode::AttachWithEnableOnExec {
                    builder = builder.enable_on_exec();
                }
                let perf = builder.open()?;

                perf_events.push((None, perf));
            }
        } else {
            for cpu in 0..cpu_count as u32 {
                for &tid in &threads {
                    let mut builder = Perf::build()
                        .pid(tid)
                        .only_cpu(cpu as _)
                        .frequency(self.frequency as u64)
                        .sample_user_stack(self.stack_size)
                        .sample_user_regs(self.regs_mask)
                        .sample_kernel()
                        .gather_context_switches()
                        .event_source(self.event_source)
                        .inherit_to_children()
                        .start_disabled();
                    if attach_mode == AttachMode::AttachWithEnableOnExec {
                        builder = builder.enable_on_exec();
                    }
                    let perf = builder.open()?;

                    perf_events.push((Some(cpu), perf));
                }
            }
        }

        for (_cpu, perf) in perf_events {
            self.members.insert(perf.fd(), Member::new(perf));
        }

        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn enable(&mut self) {
        for perf in self.members.values_mut() {
            perf.enable();
        }

        self.stopped_processes.clear();
    }

    pub fn wait(&mut self) {
        for member in self.members.values() {
            if member.are_events_pending() {
                return;
            }
        }

        poll_events(&mut self.poll_fds, self.members.values());
    }

    pub fn iter(&mut self) -> vec::Drain<EventRef> {
        self.event_buffer.clear();

        let mut fds_to_remove = Vec::new();
        for member in self.members.values_mut() {
            let perf = &mut member.perf;
            if !perf.are_events_pending() {
                if member.is_closed.get() {
                    fds_to_remove.push(perf.fd());
                    continue;
                }

                continue;
            }

            self.event_buffer.extend(perf.iter());
        }

        for fd in fds_to_remove {
            self.members.remove(&fd);
        }

        self.event_buffer.drain(..)
    }
}
