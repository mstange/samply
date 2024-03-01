use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::os::unix::io::RawFd;
use std::time::Duration;
use std::{fs, io};

use byteorder::LittleEndian;
use linux_perf_data::linux_perf_event_reader::get_record_timestamp;
use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Token};

use super::perf_event::{EventRef, EventSource, Perf};
use super::sorter::EventSorter;

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
    is_closed: bool,
}

impl Member {
    fn new(perf: Perf) -> Self {
        Member {
            perf,
            is_closed: false,
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
    event_sorter: EventSorter<RawFd, u64, EventRef>,
    members: BTreeMap<RawFd, Member>,
    poll: Poll,
    poll_events: Events,
    frequency: u32,
    stack_size: u32,
    regs_mask: u64,
    event_source: EventSource,
    stopped_processes: Vec<StoppedProcess>,
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
            event_sorter: EventSorter::new(),
            members: Default::default(),
            poll: Poll::new().unwrap(),
            poll_events: Events::with_capacity(16),
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
            let fd = perf.fd();
            self.members.insert(fd, Member::new(perf));
            self.poll.registry().register(
                &mut SourceFd(&fd),
                Token(fd as usize),
                Interest::READABLE,
            )?;
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

        let result = self
            .poll
            .poll(&mut self.poll_events, Some(Duration::from_millis(100)));
        if let Err(err) = result {
            eprintln!("poll failed: {}", err);
            return;
        }

        for ev in self.poll_events.iter() {
            if ev.is_read_closed() {
                let fd = ev.token().0 as RawFd;
                self.members.get_mut(&fd).unwrap().is_closed = true;
            }
        }
    }

    pub fn consume_events(&mut self, cb: &mut impl FnMut(EventRef)) {
        let mut fds_to_remove = Vec::new();
        loop {
            for (&fd, member) in &mut self.members {
                self.event_sorter.begin_group(fd);
                while let Some(ev) = self.event_sorter.pop() {
                    cb(ev);
                }

                let perf = &mut member.perf;
                if !perf.are_events_pending() {
                    if member.is_closed {
                        fds_to_remove.push(perf.fd());
                        continue;
                    }
                    continue;
                }

                self.event_sorter.extend(perf.iter().map(|event| {
                    let rec = event.get();
                    let timestamp = get_record_timestamp::<LittleEndian>(
                        rec.record_type,
                        rec.data,
                        &rec.parse_info,
                    )
                    .expect("All events should have a record identifier");
                    (timestamp, event)
                }));
            }

            self.event_sorter.advance_round();
            while let Some(ev) = self.event_sorter.pop() {
                cb(ev);
            }

            for fd in fds_to_remove.drain(..) {
                let result = self.poll.registry().deregister(&mut SourceFd(&fd));
                if let Err(err) = result {
                    eprintln!("deregister failed: {}", err);
                    continue;
                }
                self.members.remove(&fd);
            }

            if !self.event_sorter.has_more() {
                break;
            }
        }
    }
}
