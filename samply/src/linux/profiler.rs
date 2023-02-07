use linux_perf_data::linux_perf_event_reader::EventRecord;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::perf_event::EventSource;
use super::perf_group::{AttachMode, PerfGroup};
use super::process::SuspendedLaunchedProcess;
use crate::linux_shared::{ConvertRegs, Converter, EventInterpretation};
use crate::server::{start_server_main, ServerProps};

#[cfg(target_arch = "x86_64")]
pub type ConvertRegsNative = crate::linux_shared::ConvertRegsX86_64;

#[cfg(target_arch = "aarch64")]
pub type ConvertRegsNative = crate::linux_shared::ConvertRegsAarch64;

pub fn start_recording(
    output_file: &Path,
    command_name: OsString,
    command_args: &[OsString],
    time_limit: Option<Duration>,
    interval: Duration,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, ()> {
    // Ignore SIGINT while the subcommand is running. The signal still reaches the process
    // under observation while we continue to record it. (ctrl+c will send the SIGINT signal
    // to all processes in the foreground process group).
    let should_terminate_on_ctrl_c = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    signal_hook::flag::register_conditional_default(
        signal_hook::consts::SIGINT,
        should_terminate_on_ctrl_c.clone(),
    )
    .expect("cannot register signal handler");

    // Start a new process for the launched command and get its pid.
    // The command will not start running until we tell it to.
    let process =
        SuspendedLaunchedProcess::launch_in_suspended_state(&command_name, command_args).unwrap();
    let pid = process.pid();

    // Create a channel for the observer thread to notify the main thread once
    // profiling has been initialized and the launched process can start.
    let (s, r) = crossbeam_channel::bounded(1);

    // Launch the observer thread. This thread will manage the perf events.
    let output_file_copy = output_file.to_owned();
    let command_name_copy = command_name.to_string_lossy().to_string();
    let observer_thread = thread::spawn(move || {
        let product = command_name_copy;

        // Create the perf events, setting ENABLE_ON_EXEC.
        let (perf_group, converter) =
            init_profiler(interval, pid, AttachMode::AttachWithEnableOnExec, &product);

        // Tell the main thread to tell the child process to begin executing.
        s.send(()).unwrap();
        drop(s);

        // Create a stop flag which always stays false. We won't stop profiling until the
        // child process is done.
        // If Ctrl+C is pressed, it will reach the child process, and the child process
        // will act on it and maybe terminate. If it does, profiling stops too because
        // the main thread's wait() call below will exit.
        let stop_flag = Arc::new(AtomicBool::new(false));

        // Start profiling the process.
        run_profiler(
            perf_group,
            converter,
            &output_file_copy,
            time_limit,
            stop_flag,
        );
    });

    // We're on the main thread here and the observer thread has just been launched.

    // Wait for profiler initialization.
    let () = r.recv().unwrap();
    drop(r);

    // Now tell the child process to start executing.
    let process = match process.unsuspend_and_run() {
        Ok(process) => process,
        Err(run_err) => {
            eprintln!("Could not launch child process: {run_err}");
            std::process::exit(1)
        }
    };

    // Phew, we're profiling!

    // Wait for the child process to quit.
    // This is where the main thread spends all its time during profiling.
    let exit_status = process.wait().unwrap();

    // The child has quit.
    // From now on, we want to terminate if the user presses Ctrl+C.
    should_terminate_on_ctrl_c.store(true, std::sync::atomic::Ordering::SeqCst);

    // Now wait for the observer thread to quit. It will keep running until all
    // perf events are closed, which happens if all processes which the events
    // are attached to have quit.
    observer_thread
        .join()
        .expect("couldn't join observer thread");

    if let Some(server_props) = server_props {
        start_server_main(output_file, server_props);
    }

    Ok(exit_status)
}

pub fn start_profiling_pid(
    output_file: &Path,
    pid: u32,
    time_limit: Option<Duration>,
    interval: Duration,
    server_props: Option<ServerProps>,
) {
    // When the first Ctrl+C is received, stop recording.
    // The server launches after the recording finishes. On the second Ctrl+C, terminate the server.
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    signal_hook::flag::register_conditional_default(signal_hook::consts::SIGINT, stop.clone())
        .expect("cannot register signal handler");
    #[cfg(unix)]
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())
        .expect("cannot register signal handler");

    let output_file_copy = output_file.to_owned();
    let product = format!("PID {pid}");
    let observer_thread = thread::spawn({
        let stop = stop.clone();
        move || {
            let (perf_group, converter) =
                init_profiler(interval, pid, AttachMode::StopAttachEnableResume, &product);
            run_profiler(perf_group, converter, &output_file_copy, time_limit, stop)
        }
    });

    observer_thread
        .join()
        .expect("couldn't join observer thread");

    // If the recording was stopped due to application terminating, set the flag so that Ctrl+C
    // terminates the server.
    stop.store(true, Ordering::SeqCst);

    if let Some(server_props) = server_props {
        start_server_main(output_file, server_props);
    }
}

fn paranoia_level() -> Option<u32> {
    let level = read_string_lossy("/proc/sys/kernel/perf_event_paranoid").ok()?;
    let level = level.trim().parse::<u32>().ok()?;
    Some(level)
}

fn init_profiler(
    interval: Duration,
    pid: u32,
    attach_mode: AttachMode,
    product_name: &str,
) -> (
    PerfGroup,
    Converter<framehop::UnwinderNative<Vec<u8>, framehop::MayAllocateDuringUnwind>>,
) {
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

    let mut perf = match perf {
        Ok(perf) => perf,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            match paranoia_level() {
                Some(level) if level > 1 => {
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
                _ => {
                    // Permission denied even though parania was probably not the reason.
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
            }
        }
        Err(error) => {
            eprintln!("Failed to start profiling: {error}");
            std::process::exit(1);
        }
    };

    // eprintln!("Enabling perf events...");
    match attach_mode {
        AttachMode::StopAttachEnableResume => perf.enable(),
        AttachMode::AttachWithEnableOnExec => {
            // The perf event will get enabled automatically once the forked child process execs.
        }
    }

    let first_sample_time = 0;

    let little_endian = cfg!(target_endian = "little");
    let machine_info = uname::uname().ok();
    let interpretation = EventInterpretation {
        main_event_attr_index: 0,
        main_event_name: "cycles".to_string(),
        sampling_is_time_based: Some(interval_nanos),
        have_context_switches: true,
        sched_switch_attr_index: None,
    };

    let mut converter =
        Converter::<framehop::UnwinderNative<Vec<u8>, framehop::MayAllocateDuringUnwind>>::new(
            product_name,
            None,
            HashMap::new(),
            machine_info.as_ref().map(|info| info.release.as_str()),
            first_sample_time,
            little_endian,
            framehop::CacheNative::new(),
            None,
            interpretation,
        );

    for event in perf.take_initial_events() {
        match event {
            EventRecord::Comm(e) => {
                converter.handle_thread_name_update(e, Some(0));
            }
            EventRecord::Mmap2(e) => {
                converter.handle_mmap2(e);
            }
            _ => unreachable!(),
        }
    }

    (perf, converter)
}

fn run_profiler(
    mut perf: PerfGroup,
    mut converter: Converter<framehop::UnwinderNative<Vec<u8>, framehop::MayAllocateDuringUnwind>>,
    output_filename: &Path,
    _time_limit: Option<Duration>,
    stop: Arc<AtomicBool>,
) {
    // eprintln!("Running...");

    let mut wait = false;
    let mut pending_lost_events = 0;
    let mut total_lost_events = 0;
    loop {
        if stop.load(Ordering::SeqCst) || perf.is_empty() {
            break;
        }

        if wait {
            wait = false;
            perf.wait();
        }

        let iter = perf.iter();
        if iter.len() == 0 {
            wait = true;
            continue;
        }

        for event_ref in iter {
            let record = event_ref.get();
            let parsed_record = record.parse().unwrap();
            // debug!("Recording parsed_record: {:#?}", parsed_record);

            match parsed_record {
                EventRecord::Sample(e) => {
                    converter.handle_sample::<ConvertRegsNative>(e);
                    /*
                    } else if interpretation.sched_switch_attr_index == Some(attr_index) {
                        converter.handle_sched_switch::<C>(e);
                    }*/
                }
                EventRecord::Fork(e) => {
                    converter.handle_thread_start(e);
                }
                EventRecord::Comm(e) => {
                    converter.handle_thread_name_update(e, record.timestamp());
                }
                EventRecord::Exit(e) => {
                    converter.handle_thread_end(e);
                }
                EventRecord::Mmap(e) => {
                    converter.handle_mmap(e);
                }
                EventRecord::Mmap2(e) => {
                    converter.handle_mmap2(e);
                }
                EventRecord::ContextSwitch(e) => {
                    let common = match record.common_data() {
                        Ok(common) => common,
                        Err(_) => continue,
                    };
                    converter.handle_context_switch(e, common);
                }
                EventRecord::Lost(event) => {
                    pending_lost_events += event.count;
                    total_lost_events += event.count;
                    continue;
                }
                _ => {}
            }

            if pending_lost_events > 0 {
                // eprintln!("Pending lost events: {pending_lost_events}");
                pending_lost_events = 0;
            }
        }
    }

    if total_lost_events > 0 {
        eprintln!("Lost {total_lost_events} events.");
    }

    let profile = converter.finish();

    let output_file = File::create(output_filename).unwrap();
    let writer = BufWriter::new(output_file);
    serde_json::to_writer(writer, &profile).expect("Couldn't write JSON");
}

pub fn read_string_lossy<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let data = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}
