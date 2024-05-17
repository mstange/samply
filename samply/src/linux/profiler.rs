use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::ops::Deref;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use fxprof_processed_profile::ReferenceTimestamp;
use linux_perf_data::linux_perf_event_reader::{
    CpuMode, Endianness, EventRecord, Mmap2FileId, Mmap2InodeAndVersion, Mmap2Record, RawData,
};
use nix::sys::wait::WaitStatus;
use tokio::sync::oneshot;

use super::perf_event::EventSource;
use super::perf_group::{AttachMode, PerfGroup};
use super::proc_maps;
use super::process::SuspendedLaunchedProcess;
use crate::linux_shared::vdso::VdsoObject;
use crate::linux_shared::{
    ConvertRegs, Converter, EventInterpretation, MmapRangeOrVec, OffCpuIndicator,
};
use crate::server::{start_server_main, ServerProps};
use crate::shared::ctrl_c::CtrlC;
use crate::shared::recording_props::{
    ProcessLaunchProps, ProfileCreationProps, RecordingMode, RecordingProps,
};

#[cfg(target_arch = "x86_64")]
pub type ConvertRegsNative = crate::linux_shared::ConvertRegsX86_64;

#[cfg(target_arch = "aarch64")]
pub type ConvertRegsNative = crate::linux_shared::ConvertRegsAarch64;

pub fn start_recording(
    recording_mode: RecordingMode,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, ()> {
    let process_launch_props = match recording_mode {
        RecordingMode::All => {
            // TODO: Implement, by sudo launching a helper process which opens cpu-wide perf events
            eprintln!("Error: Profiling all processes is currently not supported on Linux.");
            eprintln!("You can profile processes which you launch via samply, or attach to a single process.");
            std::process::exit(1)
        }
        RecordingMode::Pid(pid) => {
            start_profiling_pid(pid, recording_props, profile_creation_props, server_props);
            return Ok(ExitStatus::from_raw(0));
        }
        RecordingMode::Launch(process_launch_props) => process_launch_props,
    };

    // We want to profile a child process which we are about to launch.

    let ProcessLaunchProps {
        mut env_vars,
        command_name,
        args,
        iteration_count,
    } = process_launch_props;

    if recording_props.coreclr {
        // We need to set DOTNET_PerfMapEnabled=2 in the environment if it's not already set.
        // TODO: implement unlink_aux_files for linux
        if !env_vars.iter().any(|p| p.0 == "DOTNET_PerfMapEnabled") {
            env_vars.push(("DOTNET_PerfMapEnabled".into(), "2".into()));
        }
    }

    // Ignore Ctrl+C while the subcommand is running. The signal still reaches the process
    // under observation while we continue to record it. (ctrl+c will send the SIGINT signal
    // to all processes in the foreground process group).
    let mut ctrl_c_receiver = CtrlC::observe_oneshot();

    // Start a new process for the launched command and get its pid.
    // The command will not start running until we tell it to.
    let process =
        SuspendedLaunchedProcess::launch_in_suspended_state(&command_name, &args, &env_vars)
            .unwrap();
    let pid = process.pid();

    // Create a channel for the observer thread to notify the main thread once
    // profiling has been initialized and the launched process can start.
    let (profile_another_pid_request_sender, profile_another_pid_request_receiver) =
        crossbeam_channel::bounded(2);
    let (profile_another_pid_reply_sender, profile_another_pid_reply_receiver) =
        crossbeam_channel::bounded(2);

    // Launch the observer thread. This thread will manage the perf events.
    let output_file_copy = recording_props.output_file.clone();
    let interval = recording_props.interval;
    let time_limit = recording_props.time_limit;
    let observer_thread = thread::spawn(move || {
        let unstable_presymbolicate = profile_creation_props.unstable_presymbolicate;
        let mut converter = make_converter(interval, profile_creation_props);

        // Wait for the initial pid to profile.
        let SamplerRequest::StartProfilingAnotherProcess(pid, attach_mode) =
            profile_another_pid_request_receiver.recv().unwrap()
        else {
            panic!("The first message should be a StartProfilingAnotherProcess")
        };

        // Create the perf events, setting ENABLE_ON_EXEC.
        let perf_group = init_profiler(interval, pid, attach_mode, &mut converter);

        // Tell the main thread to tell the child process to begin executing.
        profile_another_pid_reply_sender.send(true).unwrap();

        // Create a stop receiver which is never notified. We won't stop profiling until the
        // child process is done.
        // If Ctrl+C is pressed, it will reach the child process, and the child process
        // will act on it and maybe terminate. If it does, profiling stops too because
        // the main thread's wait() call below will exit.
        let (_stop_sender, stop_receiver) = oneshot::channel();

        // Start profiling the process.
        run_profiler(
            perf_group,
            converter,
            &output_file_copy,
            time_limit,
            profile_another_pid_request_receiver,
            profile_another_pid_reply_sender,
            stop_receiver,
            unstable_presymbolicate,
        );
    });

    // We're on the main thread here and the observer thread has just been launched.

    // Request profiling of our process and wait for profiler initialization.
    profile_another_pid_request_sender
        .send(SamplerRequest::StartProfilingAnotherProcess(
            pid,
            AttachMode::AttachWithEnableOnExec,
        ))
        .unwrap();
    let _ = profile_another_pid_reply_receiver.recv().unwrap();

    // Now tell the child process to start executing.
    let process = match process.unsuspend_and_run() {
        Ok(process) => process,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let command_name = command_name.to_string_lossy();
            if command_name.starts_with('-') {
                eprintln!("error: unexpected argument '{command_name}' found");
            } else {
                eprintln!("Error: Could not find an executable with the name {command_name}.");
            }
            std::process::exit(1)
        }
        Err(run_err) => {
            eprintln!("Could not launch child process: {run_err}");
            std::process::exit(1)
        }
    };

    // Phew, we're profiling!

    // Wait for the child process to quit.
    // This is where the main thread spends all its time during profiling.
    let mut wait_status = process.wait().unwrap();

    for i in 2..=iteration_count {
        let previous_run_exited_with_success = match &wait_status {
            WaitStatus::Exited(_pid, exit_code) => ExitStatus::from_raw(*exit_code).success(),
            _ => false,
        };
        if !previous_run_exited_with_success {
            eprintln!(
                "Skipping remaining iterations due to non-success exit status: {wait_status:?}"
            );
            break;
        }
        eprintln!("Running iteration {i} of {iteration_count}...");
        let process =
            SuspendedLaunchedProcess::launch_in_suspended_state(&command_name, &args, &env_vars)
                .unwrap();
        let pid = process.pid();

        // Tell the sampler to start profiling another pid, and wait for it to signal us to go ahead.
        profile_another_pid_request_sender
            .send(SamplerRequest::StartProfilingAnotherProcess(
                pid,
                AttachMode::AttachWithEnableOnExec,
            ))
            .unwrap();
        let succeeded = profile_another_pid_reply_receiver.recv().unwrap();
        if !succeeded {
            break;
        }

        // Now tell the child process to start executing.
        let process = match process.unsuspend_and_run() {
            Ok(process) => process,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let command_name = command_name.to_string_lossy();
                eprintln!("Error: Could not find an executable with the name {command_name}.");
                std::process::exit(1)
            }
            Err(run_err) => {
                eprintln!("Could not launch child process: {run_err}");
                break;
            }
        };

        wait_status = process.wait().expect("couldn't wait for child");
    }

    profile_another_pid_request_sender
        .send(SamplerRequest::StopProfilingOncePerfEventsExhausted)
        .unwrap();

    // The launched subprocess is done. From now on, we want to terminate if the user presses Ctrl+C.
    ctrl_c_receiver.close();

    // Now wait for the observer thread to quit. It will keep running until all
    // perf events are closed, which happens if all processes which the events
    // are attached to have quit.
    observer_thread
        .join()
        .expect("couldn't join observer thread");

    if let Some(server_props) = server_props {
        let profile_filename = &recording_props.output_file;
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(profile_filename).expect("Couldn't open file we just wrote"),
            profile_filename,
        )
        .expect("Couldn't parse libinfo map from profile file");

        start_server_main(profile_filename, server_props, libinfo_map);
    }

    let exit_status = match wait_status {
        WaitStatus::Exited(_pid, exit_code) => ExitStatus::from_raw(exit_code),
        _ => ExitStatus::default(),
    };
    Ok(exit_status)
}

fn start_profiling_pid(
    pid: u32,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    server_props: Option<ServerProps>,
) {
    // When the first Ctrl+C is received, stop recording.
    let ctrl_c_receiver = CtrlC::observe_oneshot();

    // Create a channel for the observer thread to notify the main thread once
    // profiling has been initialized.
    let (profile_another_pid_request_sender, profile_another_pid_request_receiver) =
        crossbeam_channel::bounded(2);
    let (profile_another_pid_reply_sender, profile_another_pid_reply_receiver) =
        crossbeam_channel::bounded(2);

    let output_file = recording_props.output_file.clone();
    let observer_thread = thread::spawn({
        move || {
            let interval = recording_props.interval;
            let time_limit = recording_props.time_limit;
            let unstable_presymbolicate = profile_creation_props.unstable_presymbolicate;
            let mut converter = make_converter(interval, profile_creation_props);
            let SamplerRequest::StartProfilingAnotherProcess(pid, attach_mode) =
                profile_another_pid_request_receiver.recv().unwrap()
            else {
                panic!("The first message should be a StartProfilingAnotherProcess")
            };
            let perf_group = init_profiler(interval, pid, attach_mode, &mut converter);

            // Tell the main thread that we are now executing.
            profile_another_pid_reply_sender.send(true).unwrap();

            let output_file = recording_props.output_file;
            run_profiler(
                perf_group,
                converter,
                &output_file,
                time_limit,
                profile_another_pid_request_receiver,
                profile_another_pid_reply_sender,
                ctrl_c_receiver,
                unstable_presymbolicate,
            )
        }
    });

    // We're on the main thread here and the observer thread has just been launched.

    // Request profiling of our process and wait for profiler initialization.
    profile_another_pid_request_sender
        .send(SamplerRequest::StartProfilingAnotherProcess(
            pid,
            AttachMode::StopAttachEnableResume,
        ))
        .unwrap();
    let _ = profile_another_pid_reply_receiver.recv().unwrap();

    // Now that we know that profiler initialization has succeeded, tell the user about it.
    eprintln!("Recording process with PID {pid} until Ctrl+C...");

    profile_another_pid_request_sender
        .send(SamplerRequest::StopProfilingOncePerfEventsExhausted)
        .unwrap();

    // Now wait for the observer thread to quit. It will keep running until the
    // CtrlC receiver has been notified, or until all perf events are closed,
    // which happens if all processes which the events are attached to have quit.
    observer_thread
        .join()
        .expect("couldn't join observer thread");

    // From now on, pressing Ctrl+C will kill our process, because the observer will have
    // dropped its CtrlC receiver by now.

    if let Some(server_props) = server_props {
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(&output_file).expect("Couldn't open file we just wrote"),
            &output_file,
        )
        .expect("Couldn't parse libinfo map from profile file");

        start_server_main(&output_file, server_props, libinfo_map);
    }
}

fn paranoia_level() -> Option<u32> {
    let level = read_string_lossy("/proc/sys/kernel/perf_event_paranoid").ok()?;
    let level = level.trim().parse::<u32>().ok()?;
    Some(level)
}

fn make_converter(
    interval: Duration,
    profile_creation_props: ProfileCreationProps,
) -> Converter<framehop::UnwinderNative<MmapRangeOrVec, framehop::MayAllocateDuringUnwind>> {
    let interval_nanos = if interval.as_nanos() > 0 {
        interval.as_nanos() as u64
    } else {
        1_000_000 // 1 million nano seconds = 1 milli second
    };

    let first_sample_time = 0;

    let endian = if cfg!(target_endian = "little") {
        Endianness::LittleEndian
    } else {
        Endianness::BigEndian
    };
    let machine_info = uname::uname().ok();
    let interpretation = EventInterpretation {
        main_event_attr_index: 0,
        main_event_name: "cycles".to_string(),
        sampling_is_time_based: Some(interval_nanos),
        off_cpu_indicator: Some(OffCpuIndicator::ContextSwitches),
        sched_switch_attr_index: None,
        known_event_indices: HashMap::new(),
        event_names: vec!["cycles".to_string()],
    };

    Converter::<framehop::UnwinderNative<MmapRangeOrVec, framehop::MayAllocateDuringUnwind>>::new(
        &profile_creation_props,
        ReferenceTimestamp::from_system_time(SystemTime::now()),
        None,
        HashMap::new(),
        machine_info.as_ref().map(|info| info.release.as_str()),
        first_sample_time,
        endian,
        framehop::CacheNative::new(),
        None,
        interpretation,
        None,
    )
}

fn init_profiler(
    interval: Duration,
    pid: u32,
    attach_mode: AttachMode,
    converter: &mut Converter<
        framehop::UnwinderNative<MmapRangeOrVec, framehop::MayAllocateDuringUnwind>,
    >,
) -> PerfGroup {
    let interval_nanos = if interval.as_nanos() > 0 {
        interval.as_nanos() as u64
    } else {
        1_000_000 // 1 million nano seconds = 1 milli second
    };

    let frequency = (1_000_000_000 / interval_nanos) as u32;
    let stack_size = 32000;
    let regs_mask = ConvertRegsNative::regs_mask();

    let perf = PerfGroup::open(
        pid,
        frequency,
        stack_size,
        EventSource::HwCpuCycles,
        regs_mask,
        attach_mode,
    );

    if let Err(error) = &perf {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            if let Some(level) = paranoia_level() {
                if level > 1 {
                    eprintln!();
                    eprintln!(
                        "'/proc/sys/kernel/perf_event_paranoid' is currently set to {level}."
                    );
                    eprintln!("In order for samply to work with a non-root user, this level needs");
                    eprintln!("to be set to 1 or lower.");
                    eprintln!("You can execute the following command and then try again:");
                    eprintln!("    echo '1' | sudo tee /proc/sys/kernel/perf_event_paranoid");
                    eprintln!();
                    std::process::exit(1);
                }
            }
        }
    }

    let mut perf = match perf {
        Ok(perf) => perf,
        Err(_) => {
            // We've already checked for permission denied due to paranoia
            // level, and exited with a warning in that case.

            // Another reason for the error could be the type of perf event:
            // The "Hardware CPU cycles" event is not supported in some contexts, for example in VMs.
            // Try a different event type.
            let perf = PerfGroup::open(
                pid,
                frequency,
                stack_size,
                EventSource::SwCpuClock,
                regs_mask,
                attach_mode,
            );
            match perf {
                Ok(perf) => perf, // Success!
                Err(error) => {
                    eprintln!("Failed to start profiling: {error}");
                    std::process::exit(1);
                }
            }
        }
    };

    // TODO: Gather threads / processes recursively, here and in PerfGroup setup.
    for entry in std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .flatten()
    {
        let tid: u32 = entry.file_name().to_string_lossy().parse().unwrap();
        let comm_path = format!("/proc/{pid}/task/{tid}/comm");
        if let Ok(buffer) = std::fs::read(comm_path) {
            let length = memchr::memchr(b'\0', &buffer).unwrap_or(buffer.len());
            let name = std::str::from_utf8(&buffer[..length]).unwrap().trim_end();
            converter.register_existing_thread(pid as i32, tid as i32, name);
        }
    }

    let maps = read_string_lossy(format!("/proc/{pid}/maps")).expect("couldn't read proc maps");
    let maps = proc_maps::parse(&maps);

    let vdso_file_id = VdsoObject::shared_instance_for_this_process()
        .map(|vdso| Mmap2FileId::BuildId(vdso.build_id().to_owned()));

    for region in maps {
        let mut protection = 0;
        if region.is_read {
            protection |= libc::PROT_READ;
        }
        if region.is_write {
            protection |= libc::PROT_WRITE;
        }
        if region.is_executable {
            protection |= libc::PROT_EXEC;
        }

        let mut flags = 0;
        if region.is_shared {
            flags |= libc::MAP_SHARED;
        } else {
            flags |= libc::MAP_PRIVATE;
        }

        let file_id = match (region.name.deref(), vdso_file_id.as_ref()) {
            ("[vdso]", Some(vdso_file_id)) => vdso_file_id.clone(),
            _ => Mmap2FileId::InodeAndVersion(Mmap2InodeAndVersion {
                major: region.major,
                minor: region.minor,
                inode: region.inode,
                inode_generation: 0,
            }),
        };

        converter.handle_mmap2(
            Mmap2Record {
                pid: pid as i32,
                tid: pid as i32,
                address: region.start,
                length: region.end - region.start,
                page_offset: region.file_offset,
                file_id,
                protection: protection as _,
                flags: flags as _,
                path: RawData::Single(&region.name.into_bytes()),
                cpu_mode: CpuMode::User,
            },
            0,
        );
    }

    // eprintln!("Enabling perf events...");
    match attach_mode {
        AttachMode::StopAttachEnableResume => perf.enable(),
        AttachMode::AttachWithEnableOnExec => {
            // The perf event will get enabled automatically once the forked child process execs.
        }
    }

    perf
}

enum SamplerRequest {
    StartProfilingAnotherProcess(u32, AttachMode),
    StopProfilingOncePerfEventsExhausted,
}

#[allow(clippy::too_many_arguments)]
fn run_profiler(
    mut perf: PerfGroup,
    mut converter: Converter<
        framehop::UnwinderNative<MmapRangeOrVec, framehop::MayAllocateDuringUnwind>,
    >,
    output_filename: &Path,
    _time_limit: Option<Duration>,
    more_processes_request_receiver: Receiver<SamplerRequest>,
    more_processes_reply_sender: Sender<bool>,
    mut stop_receiver: oneshot::Receiver<()>,
    unstable_presymbolicate: bool,
) {
    // eprintln!("Running...");

    let mut should_stop_profiling_once_perf_events_exhausted = false;
    let mut pending_lost_events = 0;
    let mut total_lost_events = 0;
    let mut last_timestamp = 0;
    loop {
        if stop_receiver.try_recv().is_ok() {
            break;
        }

        match more_processes_request_receiver.try_recv() {
            Ok(SamplerRequest::StartProfilingAnotherProcess(another_pid, attach_mode)) => {
                match perf.open_process(another_pid, attach_mode) {
                    Ok(_) => {
                        more_processes_reply_sender.send(true).unwrap();
                    }
                    Err(error) => {
                        eprintln!("Failed to start profiling on subsequent process: {error}");
                        more_processes_reply_sender.send(false).unwrap();
                    }
                }
            }
            Ok(SamplerRequest::StopProfilingOncePerfEventsExhausted) => {
                should_stop_profiling_once_perf_events_exhausted = true;
            }
            Err(_) => {
                // No requests pending at the moment.
            }
        }

        if perf.is_empty() && !should_stop_profiling_once_perf_events_exhausted {
            match more_processes_request_receiver.recv() {
                Ok(SamplerRequest::StartProfilingAnotherProcess(another_pid, attach_mode)) => {
                    match perf.open_process(another_pid, attach_mode) {
                        Ok(_) => {
                            more_processes_reply_sender.send(true).unwrap();
                        }
                        Err(error) => {
                            eprintln!("Failed to start profiling on subsequent process: {error}");
                            more_processes_reply_sender.send(false).unwrap();
                        }
                    }
                }
                Ok(SamplerRequest::StopProfilingOncePerfEventsExhausted) => {
                    should_stop_profiling_once_perf_events_exhausted = true;
                }
                Err(_) => {
                    // No requests pending at the moment.
                }
            }
        }

        if perf.is_empty() && should_stop_profiling_once_perf_events_exhausted {
            break;
        }

        perf.consume_events(&mut |event_ref| {
            let record = event_ref.get();
            let parsed_record = record.parse().unwrap();
            // debug!("Recording parsed_record: {:#?}", parsed_record);

            if let Some(timestamp) = record.timestamp() {
                if timestamp < last_timestamp {
                    // eprintln!(
                    //     "bad timestamp ordering; {timestamp} is earlier but arrived after {last_timestamp}"
                    // );
                }
                last_timestamp = timestamp;
            }

            match parsed_record {
                EventRecord::Sample(e) => {
                    converter.handle_main_event_sample::<ConvertRegsNative>(&e);
                    /*
                    } else if interpretation.sched_switch_attr_index == Some(attr_index) {
                        converter.handle_sched_switch_sample::<C>(e);
                    }*/
                }
                EventRecord::Fork(e) => {
                    converter.handle_fork(e);
                }
                EventRecord::Comm(e) => {
                    converter.handle_comm(e, record.timestamp());
                }
                EventRecord::Exit(e) => {
                    converter.handle_exit(e);
                }
                EventRecord::Mmap(e) => {
                    converter.handle_mmap(e, last_timestamp);
                }
                EventRecord::Mmap2(e) => {
                    converter.handle_mmap2(e, last_timestamp);
                }
                EventRecord::ContextSwitch(e) => {
                    let common = match record.common_data() {
                        Ok(common) => common,
                        Err(_) => return,
                    };
                    converter.handle_context_switch(e, common);
                }
                EventRecord::Lost(event) => {
                    pending_lost_events += event.count;
                    total_lost_events += event.count;
                    return;
                }
                _ => {}
            }

            if pending_lost_events > 0 {
                // eprintln!("Pending lost events: {pending_lost_events}");
                pending_lost_events = 0;
            }
        });

        perf.wait();
    }

    if total_lost_events > 0 {
        eprintln!("Lost {total_lost_events} events.");
    }

    let profile = converter.finish();

    {
        let output_file = File::create(output_filename).unwrap();
        let writer = BufWriter::new(output_file);
        serde_json::to_writer(writer, &profile).expect("Couldn't write JSON");
    }

    if unstable_presymbolicate {
        crate::shared::symbol_precog::presymbolicate(
            &profile,
            &output_filename.with_extension("syms.json"),
        );
    }
}

pub fn read_string_lossy<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let data = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}
