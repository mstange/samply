#![allow(dead_code)]
#![allow(unused_imports)]

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::BufWriter;
use std::mem::size_of;
use std::ops::DerefMut;
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use debugid::DebugId;
use serde_json::to_writer;
use tokio::runtime;

use crate::server::{ServerProps, start_server_main};
use crate::shared::recording_props::{ProfileCreationProps, ProcessLaunchProps, RecordingProps};

use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, CpuDelta, Frame, FrameFlags, FrameInfo,
                               LibraryHandle, LibraryInfo, ProcessHandle, Profile,
                               ReferenceTimestamp, ThreadHandle, Timestamp};

use ferrisetw::{EventRecord, FileTrace, SchemaLocator};
use ferrisetw::trace::TraceTrait;
use uuid::Uuid;

use windows::{
    Win32::Foundation::MAX_PATH,
    Win32::System::ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverFileNameW},
};
use windows::Win32::System::Diagnostics::Etw::QueryTraceProcessingHandle;
use wholesym::SymbolManager;

use super::etw::*;

use crate::windows::winutils;

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
    // we need the debug privilege token in order to get the kernel's address and run xperf.
    winutils::enable_debug_privilege();

    let timebase = std::time::SystemTime::now();
    let timebase = ReferenceTimestamp::from_system_time(timebase);

    //let mut jit_category_manager = crate::shared::jit_category_manager::JitCategoryManager::new();

    let profile = Profile::new(
        &profile_creation_props.profile_name,
        timebase,
        recording_props.interval.into(),
    );

    let rt = runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    let profile = Arc::new(Mutex::new(profile));

    let mut context = ProfileContext::new(profile.clone(), rt.handle().clone());

    context.add_kernel_drivers();

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

        context.add_interesting_pid(child.id());

        let exit_status = child.wait().unwrap();
        if !exit_status.success() {
            eprintln!("Child process exited with {:?}", exit_status);
        }
    }

    context.stop_xperf();

    eprintln!("Processing ETL trace...");
    let etl_file = context.etl_file.clone().unwrap();
    let (trace, handle) = FileTrace::new(etl_file.clone(), move |ev, sl| {
        trace_callback(ev, sl, &mut context);
    }).start().unwrap();

    // TODO: grab the header info, so that we can pull out StartTime (and PerfFreq). Not really important.
    // QueryTraceProcessingHandle(handle, EtwQueryLogFileHeader, None, 0, &ptr to TRACE_LOGFILE_HEADER)

    FileTrace::process_from_handle(handle).unwrap();

    let n_events = trace.events_handled();
    eprintln!("Read {} events from file", n_events);

    // delete etl_file
    std::fs::remove_file(&etl_file).expect(format!("Failed to delete ETL file {:?}", etl_file.to_str().unwrap()).as_str());

    // write the profile to a json file
    let output_file = recording_props.output_file.clone();

    let file = File::create(&output_file).unwrap();
    let writer = BufWriter::new(file);
    to_writer(writer, profile.lock().unwrap().deref_mut()).expect("Couldn't write JSON");

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

struct ProfileContext {
    profile: Arc<Mutex<Profile>>,
    rt: runtime::Handle,
    timebase_nanos: u64,

    // state -- keep track of the processes etc we've seen as we're processing,
    // and their associated handles in the json profile
    processes: HashMap<u32, ProcessHandle>,
    threads: HashMap<u32, ThreadHandle>,
    seen_first_thread_for_process: HashSet<u32>,
    libs: HashMap<String, LibraryHandle>,

    // These are the processes + their children that we want to write into
    // the profile.json. If it's empty, trace everything.
    interesting_processes: HashSet<u32>,

    // default categories
    default_category: CategoryPairHandle,
    kernel_category: CategoryPairHandle,

    // cache of device mappings
    device_mappings: HashMap<String, String>, // map of \Device\HarddiskVolume4 -> C:\

    // the minimum address for kernel drivers, so that we can assign kernel_category to the frame
    // TODO why is this needed -- kernel libs are at global addresses, why do I need to indicate
    // this per-frame; shouldn't there be some kernel override?
    kernel_min: u64,

    // architecture to record in the trace. will be the system architecture for now.
    // TODO no idea how to handle "I'm on aarch64 windows but I'm recording a win64 process".
    // I have no idea how stack traces work in that case anyway, so this is probably moot.
    arch: String,

    // the ETL file we're either recording to or parsing from
    etl_file: Option<PathBuf>,
}

impl ProfileContext {
    fn new(profile: Arc<Mutex<Profile>>, rt: runtime::Handle) -> Self {
        let default_category = CategoryPairHandle::from(profile.lock().unwrap().add_category("User", CategoryColor::Yellow));
        let kernel_category = CategoryPairHandle::from(profile.lock().unwrap().add_category("Kernel", CategoryColor::Red));
        Self {
            profile,
            rt,
            timebase_nanos: 0,
            processes: HashMap::new(),
            threads: HashMap::new(),
            seen_first_thread_for_process: HashSet::new(),
            libs: HashMap::new(),
            interesting_processes: HashSet::new(),
            default_category,
            kernel_category,
            device_mappings: winutils::get_dos_device_mappings(),
            kernel_min: u64::MAX,
            arch: "aarch64".to_string(),
            etl_file: None,
        }
    }

    fn add_interesting_pid(&mut self, pid: u32) {
        self.interesting_processes.insert(pid);
    }

    fn is_interesting_pid(&self, pid: u32) -> bool {
        // if we didn't flag anything as interesting, trace everything
        self.interesting_processes.is_empty() ||
        // or if we have explicit ones, trace those
        self.interesting_processes.contains(&pid) ||
        // or if we've already decided to trace it
        self.processes.contains_key(&pid)
    }

    fn new_with_existing_recording(profile: Arc<Mutex<Profile>>, rt: runtime::Handle, etl_file: &Path) -> Self {
        let mut context = Self::new(profile, rt);
        context.etl_file = Some(PathBuf::from(etl_file));
        context
    }

    fn add_kernel_drivers(&mut self) {
        for (path, start_avma, end_avma) in winutils::iter_kernel_drivers() {
            if self.kernel_min == u64::MAX  {
                // take the first as the start; iter_kernel_drivers is sorted
                self.kernel_min = start_avma;
            }

            let path = self.map_device_path(&path);
            eprintln!("kernel driver: {} {:x} {:x}", path, start_avma, end_avma);
            let lib_info = self.library_info_for_path(&path);
            let lib_handle = self.profile.lock().unwrap().add_lib(lib_info);
            self.profile.lock().unwrap().add_kernel_lib_mapping(lib_handle, start_avma, end_avma, 0);
        }
    }

    fn map_device_path(&self, path: &str) -> String {
        for (k, v) in &self.device_mappings {
            if path.starts_with(k) {
                let r = format!("{}{}", v, path.split_at(k.len()).1);
                return r;
            }
        }
        path.into()
    }

    fn library_info_for_path(&self, path: &str) -> LibraryInfo {
        let path = self.map_device_path(path);

        // TODO -- I'm not happy about this. I'd like to be able to just reprocess these before we write out the profile,
        // instead of blocking during processing the samples. But we're postprocessing anyway, so not a big deal.
        if let Ok(info) = self.rt.block_on(SymbolManager::library_info_for_binary_at_path(path.as_ref(), None)) {
            LibraryInfo {
                name: info.name.unwrap(),
                path: info.path.unwrap(),
                debug_name: info.debug_name.unwrap_or(path.to_string()),
                debug_path: info.debug_path.unwrap_or(path.to_string()),
                debug_id: info.debug_id.unwrap_or(Default::default()),
                code_id: None,
                arch: info.arch,
                symbol_table: None,
            }
        } else {
            // Not found; put in a dummy
            LibraryInfo {
                name: path.to_string(),
                path: path.to_string(),
                debug_name: path.to_string(),
                debug_path: path.to_string(),
                debug_id: DebugId::from_uuid(Uuid::new_v4()),
                code_id: None,
                arch: Some(self.arch.clone()),
                symbol_table: None,
            }
        }
    }

    fn start_xperf(&mut self, output_file: &Path) {
        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let etl_file = format!("{}.etl", output_file.to_str().unwrap());
        let mut xperf = std::process::Command::new("xperf");
        xperf.arg("-on");
        xperf.arg("PROC_THREAD+LOADER+PROFILE");
        xperf.arg("-stackwalk");
        xperf.arg("profile");
        // Virtualised ARM64 Windows crashes out on PROFILE tracing, and that's what I'm developing
        // on, so these are hacky args to get me a useful profile that I can work with.
        //xperf.arg("PROC_THREAD+LOADER+CSWITCH+SYSCALL+VIRT_ALLOC+OB_HANDLE");
        //xperf.arg("-stackwalk");
        //xperf.arg("VirtualAlloc+VirtualFree+HandleCreate+HandleClose");
        xperf.arg("-f");
        xperf.arg(&etl_file);

        let _ = xperf.spawn()
            .unwrap_or_else(|err| {
                panic!("failed to execute xperf: {}", err);
            })
            .wait()
            .is_ok_and(|exitstatus| {
                if !exitstatus.success() {
                    panic!("xperf exited with: {:?}", exitstatus);
                }
                true
            });

        eprintln!("xperf session running...");

        self.etl_file = Some(PathBuf::from(&etl_file));
    }

    fn stop_xperf(&mut self) {
        let mut xperf = std::process::Command::new("xperf");
        xperf.arg("-stop");

        xperf.spawn()
            .unwrap_or_else(|err| {
                panic!("failed to execute xperf: {}", err);
            })
            .wait()
            .expect("Failed to wait on xperf");

        eprintln!("xperf session stopped.");
    }
}

fn trace_callback(ev: &EventRecord, sl: &SchemaLocator, context: &mut ProfileContext)
{
    let mut profile = context.profile.lock().unwrap();

    // For the first event we see, use its time as the reference. Maybe there's something
    // on the trace itself we can use.
    // TODO see comment earlier about using QueryTraceProcessingHandle
    if context.timebase_nanos == 0 {
        context.timebase_nanos = ev.raw_timestamp() as u64;
    }

    let Some((ts, event)) = get_tracing_event(ev, sl) else { return };

    let timestamp = Timestamp::from_nanos_since_reference(ts - context.timebase_nanos);
    //eprintln!("{} {:?}", ts, event);
    match event {
        TracingEvent::ProcessDCStart(e) |
        TracingEvent::ProcessStart(e) => {
            let exe = e.ImageFileName.unwrap();
            let pid = e.ProcessId.unwrap();
            let ppid = e.ParentId.unwrap_or(0);

            if context.is_interesting_pid(pid)  || context.is_interesting_pid(ppid) {
                let handle = profile.add_process(&exe, pid, timestamp);
                context.processes.insert(pid, handle);
            }
        }
        TracingEvent::ProcessStop(e) => {
            let pid = e.ProcessId.unwrap();
            if let Some(handle) = context.processes.remove(&pid) {
                profile.set_process_end_time(handle, timestamp);
                context.seen_first_thread_for_process.remove(&pid);
            }
        }
        TracingEvent::ThreadDCStart(e) |
        TracingEvent::ThreadStart(e) => {
            let pid = e.ProcessId.unwrap();
            let tid = e.TThreadId.unwrap();

            if let Some(process_handle) = context.processes.get(&pid) {
                let is_main = !context.seen_first_thread_for_process.contains(&pid);
                if is_main {
                    context.seen_first_thread_for_process.insert(pid);
                }
                let handle = profile.add_thread(*process_handle, tid, timestamp, is_main);
                context.threads.insert(tid, handle);
            }
        }
        TracingEvent::ThreadStop(e) => {
            let tid = e.TThreadId.unwrap();

            if let Some(handle) = context.threads.remove(&tid) {
                profile.set_thread_end_time(handle, timestamp);
            }
        }
        TracingEvent::ImageDCLoad(e) |
        TracingEvent::ImageLoad(e) => {
            let pid = e.ProcessId.unwrap();
            let base = e.ImageBase.unwrap();
            let size = e.ImageSize.unwrap();
            let filename = e.FileName.unwrap();

            if let Some(process_handle) = context.processes.get(&pid) {
                let lib_handle =
                    if let Some(&lib_handle) = context.libs.get(&filename) {
                        lib_handle
                    } else {
                        let lib_handle = profile.add_lib(context.library_info_for_path(&filename));
                        //eprintln!("image: {} {} {:x} {:x}", pid, filename, base, size);
                        context.libs.insert(filename.clone(), lib_handle);
                        lib_handle
                    };

                profile.add_lib_mapping(*process_handle, lib_handle, base, base + size, 0);
            }
        }
        TracingEvent::ImageUnload(e) => {
            let pid = e.ProcessId.unwrap();
            let base = e.ImageBase.unwrap();

            if let Some(process_handle) = context.processes.get(&pid) {
                profile.remove_lib_mapping(*process_handle, base);
            }
        }
        TracingEvent::StackWalk(e) => {
            let _pid = e.StackProcess;
            let tid = e.StackThread;

            if let Some(&thread_handle) = context.threads.get(&tid) {
                let frames = e.Stack.iter()
                    .take_while(|&&frame| frame != 0)
                    .map(|&frame| {
                        FrameInfo {
                            frame: Frame::InstructionPointer(frame),
                            flags: FrameFlags::empty(),
                            category_pair: if frame >= context.kernel_min { context.kernel_category } else { context.default_category },
                        }
                    });

                profile.add_sample(thread_handle, timestamp, frames.into_iter(),
                                   CpuDelta::ZERO, 1);
            }
        }
    }

    // Haven't seen extended data in anything yet. Not used by kernel logger I don't think.
    //for edata in ev.extended_data().iter() {
    //    eprintln!("extended data: {:?} {:?}", edata.data_type(), edata.to_extended_data_item());
    //}
}
