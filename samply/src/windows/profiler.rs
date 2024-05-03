#![allow(dead_code)]
#![allow(unused_imports)]

use serde_json::to_writer;
use std::fs::File;
use std::io::BufWriter;
use std::ops::DerefMut;
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::AtomicPtr;
use std::sync::{Arc, Mutex};

use crate::server::{start_server_main, ServerProps};
use crate::shared::recording_props::{ProcessLaunchProps, ProfileCreationProps, RecordingProps};

use fxprof_processed_profile::{Profile, ReferenceTimestamp, SamplingInterval};

use crate::windows::profile_context::ProfileContext;
use crate::windows::{etw_gecko, winutils};

// Hello intrepid explorer! You may be in this code because you'd like to extend something,
// or are trying to figure out how various ETW things work. It's not the easiest API!
//
// Here are some useful things I've discovered along the way:
// - The useful ETW events are part of the Kernel provider. This Kernel provider uses "classic" "MOF" events,
//   not new-style XML manifest events. This makes them a bit more of a pain to work with, and they're
//   poorly documented. Luckily, ferrisetw does a good job of pulling out the schema even for MOF events.
// - When trying to decipher ETW providers, opcodes, keywords, etc. there are two tools that
//   are useful:
//   - `logman query providers Microsoft-Windows-Kernel-Process` (or any other provider)
//     will give you info about the provider, including its keywords and values. Handy for a quick view.
//   - `Get-WinEvent -ListProvider "Microsoft-Windows-Kernel-Process` (PowerShell) will
//     give you more info. The returned object contains everything you may want to know, but you'll
//     need to print it. For example:
//       - `(Get-WinEvent -ListProvider "Microsoft-Windows-Kernel-Process").Opcodes`
//          will give you the opcodes.
//       - `(Get-WinEvent -ListProvider "Microsoft-Windows-Kernel-Process").Events[5]` will give you details about
//          that event.
//   - To get information about them, you can use `wbemtest`. Connect to the default store, and run a query like
//     `SELECT * FROM meta_class WHERE __THIS ISA "Win32_SystemTrace"`, Double click that to get object information,
//     including decompiling to MOF (the actual struct). There's a class hierarchy here.
//   - But not all events show up in `wbemtest`! A MOF file is in the Windows WDK, "Include/10.0.22621.0/km/wmicore.mof".
//     But it seems to only contain "32-bit" versions of events (e.g. addresses are u32, not u64). I can't find
//     a fully up to date .mof.
//   - There are some more complex StackWalk events (see etw.rs for info) but I haven't seen them.

pub fn start_profiling_pid(
    _pid: u32,
    _recording_props: RecordingProps,
    _profile_creation_props: ProfileCreationProps,
    _server_props: Option<ServerProps>,
) {
    // we need the debug privilege token in order to get the kernel's address and run xperf.
    winutils::enable_debug_privilege();

    // TODO
}

pub fn start_recording(
    process_launch_props: ProcessLaunchProps,
    recording_props: RecordingProps,
    profile_creation_props: ProfileCreationProps,
    server_props: Option<ServerProps>,
) -> Result<ExitStatus, i32> {
    let timebase = std::time::SystemTime::now();
    let timebase = ReferenceTimestamp::from_system_time(timebase);

    //let mut jit_category_manager = crate::shared::jit_category_manager::JitCategoryManager::new();

    let interval_8khz = SamplingInterval::from_nanos(122100); // 8192Hz // only with the higher recording rate?
    let profile = Profile::new(
        &profile_creation_props.profile_name,
        timebase,
        interval_8khz, // recording_props.interval.into(),
    );

    let merge_threads = false;
    let include_idle_time = false;
    let arch = get_native_arch(); // TODO: Detect from file if reading from file
    let mut context = ProfileContext::new(profile, arch, merge_threads, include_idle_time);

    // we need the debug privilege token in order to get the kernel's address and run xperf.
    ////winutils::enable_debug_privilege();
    ////context.add_kernel_drivers();

    let (etl_file, existing_etl) = if !process_launch_props
        .command_name
        .to_str()
        .unwrap()
        .ends_with(".etl")
    {
        // Start xperf.
        context.start_xperf(&recording_props.output_file);

        // Run the command.
        // !!!FIXME!!! We are in an elevated context right now. Running this will run
        // the command as Administrator, which is almost definitely not what the
        // user wanted. We could drop privileges before running the command, but
        // I think what we need to do is have the _initial_ samply session stick
        // around and act as the command executor, passing us the pids it spawns.
        // That way the command will get executed in exactly the context the user intended.
        for _ in 0..process_launch_props.iteration_count {
            let mut child = std::process::Command::new(&process_launch_props.command_name);
            child.args(&process_launch_props.args);
            child.envs(process_launch_props.env_vars.iter().map(|(k, v)| (k, v)));
            let mut child = child.spawn().unwrap();

            context.add_interesting_process_id(child.id());

            let exit_status = child.wait().unwrap();
            if !exit_status.success() {
                eprintln!("Child process exited with {:?}", exit_status);
            }
        }
        context.stop_xperf();

        (context.etl_file.clone().unwrap(), false)
    } else {
        eprintln!("Existing ETL");
        if let Some(names) = &profile_creation_props.include_process_names {
            for name in names {
                context.add_interesting_process_name(name);
            }
        }
        if let Some(ids) = &profile_creation_props.include_process_ids {
            for id in ids {
                context.add_interesting_process_id(*id);
            }
        }
        (PathBuf::from(&process_launch_props.command_name), true)
    };

    eprintln!("Processing ETL trace...");

    let output_file = recording_props.output_file.clone();

    etw_gecko::profile_pid_from_etl_file(&mut context, Path::new(&etl_file));

    // delete etl_file
    if !existing_etl {
        //std::fs::remove_file(&etl_file).expect(format!("Failed to delete ETL file {:?}", etl_file.to_str().unwrap()).as_str());
    }

    // write the profile to a json file
    let file = File::create(&output_file).unwrap();
    let writer = BufWriter::new(file);
    {
        let profile = context.profile.borrow();
        to_writer(writer, &*profile).expect("Couldn't write JSON");
    }

    // then fire up the server for the profiler front end, if not save-only
    if let Some(server_props) = server_props {
        let libinfo_map = crate::profile_json_preparse::parse_libinfo_map_from_profile_file(
            File::open(&output_file).expect("Couldn't open file we just wrote"),
            &output_file,
        )
        .expect("Couldn't parse libinfo map from profile file");

        start_server_main(&output_file, server_props, libinfo_map);
    }

    Ok(ExitStatus::from_raw(0))
}

#[cfg(target_arch = "x86")]
fn get_native_arch() -> &'static str {
    "x86"
}

#[cfg(target_arch = "x86_64")]
fn get_native_arch() -> &'static str {
    "x86_64"
}

#[cfg(target_arch = "aarch64")]
fn get_native_arch() -> &'static str {
    "arm64"
}
