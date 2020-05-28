use cocoa::base::id;
use objc::rc::autoreleasepool;
use objc::{msg_send, sel, sel_impl};
use profiler_symbol_server::start_server;
use serde_json::to_writer;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::{thread, time};
use structopt::StructOpt;
use which::which;

mod dyld_bindings;
mod gecko_profile;
mod proc_maps;
mod process_launcher;

use gecko_profile::ProfileBuilder;
use proc_maps::{get_dyld_info, DyldInfo};
use process_launcher::{mach_port_t, MachError, ProcessLauncher, Wait};

#[cfg(target_os = "macos")]
#[link(name = "Symbolication", kind = "framework")]
extern "C" {
    #[no_mangle]
    #[link_name = "OBJC_CLASS_$_VMUSampler"]
    static VMUSampler_class: objc::runtime::Class;
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "perfrecord",
    about = r#"Run a command and record a CPU profile of its execution.

EXAMPLES:
    perfrecord ./yourcommand args
    perfrecord --launch-when-done ./yourcommand args
    perfrecord -o prof.json ./yourcommand args
    perfrecord --launch prof.json"#
)]
struct Opt {
    /// Launch the profiler and display the collected profile after recording.
    #[structopt(long = "launch-when-done")]
    launch_when_done: bool,

    /// When recording, limit the recorded time to the specified number of seconds
    #[structopt(short = "t", long = "time-limit")]
    time_limit: Option<f64>,

    /// When recording, save the collected profile to this file.
    #[structopt(
        short = "o",
        long = "out",
        default_value = "profile.json",
        parse(from_os_str)
    )]
    output_file: PathBuf,

    /// If neither --launch nor --serve are specified, profile this command.
    #[structopt(subcommand)]
    rest: Option<Subcommands>,

    /// Launch the profiler in your default browser and display the selected profile.
    #[structopt(short = "l", long = "launch", parse(from_os_str))]
    file_to_launch: Option<PathBuf>,

    /// Serve the specified profile from a local webserver but do not open the browser.
    #[structopt(short = "s", long = "serve", parse(from_os_str))]
    file_to_serve: Option<PathBuf>,
}

#[derive(Debug, PartialEq, StructOpt)]
enum Subcommands {
    #[structopt(external_subcommand)]
    Command(Vec<String>),
}

fn main() -> Result<(), MachError> {
    let opt = Opt::from_args();
    let open_in_browser = opt.file_to_launch.is_some();
    let file_for_launching_or_serving = opt.file_to_launch.or(opt.file_to_serve);
    if let Some(file) = file_for_launching_or_serving {
        start_server_main(&file, open_in_browser);
        return Ok(());
    }
    if let Some(Subcommands::Command(command)) = opt.rest {
        if !command.is_empty() {
            start_recording(
                &opt.output_file,
                &command,
                opt.time_limit,
                opt.launch_when_done,
            )?;
            return Ok(());
        }
    }
    println!("Error: missing command\n");
    Opt::clap().print_help().unwrap();
    std::process::exit(1);
}

#[tokio::main]
async fn start_server_main(file: &Path, open_in_browser: bool) {
    start_server(file, open_in_browser).await;
}

fn start_recording(
    output_file: &Path,
    args: &[String],
    time_limit: Option<f64>,
    launch_when_done: bool,
) -> Result<(), MachError> {
    let env: Vec<_> = std::env::vars()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    let env: Vec<&str> = env.iter().map(std::ops::Deref::deref).collect();

    let command = which(args.first().unwrap()).expect("Couldn't resolve command name");
    let args: Vec<&str> = args.iter().map(std::ops::Deref::deref).collect();

    let mut launcher = ProcessLauncher::new(&command, &args, &env)?;
    let child_pid = launcher.get_pid();
    let child_task = launcher.take_task();
    println!("child PID: {}, childTask: {}\n", child_pid, child_task);

    let dyld_info = get_dyld_info(child_task).expect("get_dyld_info failed");

    let sampler = Sampler::new_with_task(child_task, time_limit.or(Some(5000.0)), 0.001, true);
    sampler.start();

    thread::sleep(time::Duration::from_millis(100));

    launcher.start_execution();

    sampler.wait_until_done();
    let samples = sampler.get_samples();

    let mut profile_builder = ProfileBuilder::new();
    for DyldInfo {
        file,
        uuid,
        address,
        vmsize,
    } in dyld_info
    {
        let uuid = match uuid {
            Some(uuid) => uuid,
            None => continue,
        };
        let name = Path::new(&file).file_name().unwrap().to_str().unwrap();
        let address_range = address..(address + vmsize);
        profile_builder.add_lib(&name, &file, &uuid, &address_range);
    }
    for Sample {
        timestamp,
        thread_index,
        frames,
        ..
    } in &samples
    {
        profile_builder.add_sample(*thread_index, *timestamp * 1000.0, frames);
    }
    let file = File::create(output_file).unwrap();
    to_writer(file, &profile_builder.to_json()).expect("Couldn't write JSON");
    // println!("profile: {:?}", profile_builder);

    if launch_when_done {
        start_server_main(output_file, true);
    }

    child_pid.wait();

    Ok(())
}

struct Sampler {
    vmu_sampler: id,
}

#[derive(Debug)]
struct Sample {
    timestamp: f64,
    thread_index: u32,
    thread_state: i32,
    frames: Vec<u64>,
}

impl Sampler {
    pub fn new_with_task(
        task: mach_port_t,
        time_limit: Option<f64>,
        interval: f64,
        all_thread_states: bool,
    ) -> Self {
        let vmu_sampler: id = unsafe { msg_send![&VMUSampler_class, alloc] };
        let vmu_sampler: id = unsafe { msg_send![vmu_sampler, initWithTask:task options:0] };
        if let Some(time_limit) = time_limit {
            let _: () = unsafe { msg_send![vmu_sampler, setTimeLimit: time_limit] };
        }
        let _: () = unsafe { msg_send![vmu_sampler, setSamplingInterval: interval] };
        let _: () = unsafe { msg_send![vmu_sampler, setRecordThreadStates: all_thread_states] };
        Sampler { vmu_sampler }
    }

    fn start(&self) {
        let _: () = unsafe { msg_send![self.vmu_sampler, start] };
    }

    fn wait_until_done(&self) {
        let _: () = unsafe { msg_send![self.vmu_sampler, waitUntilDone] };
    }

    fn get_samples(&self) -> Vec<Sample> {
        let mut samples = Vec::new();
        autoreleasepool(|| {
            let vmu_samples: id = unsafe { msg_send![self.vmu_sampler, samples] };
            let count: u64 = unsafe { msg_send![vmu_samples, count] };
            for i in 0..count {
                let backtrace: id = unsafe { msg_send![vmu_samples, objectAtIndex: i] };

                // Yikes, for the timestamps we need to get the _callstack ivar.
                let callstack: &Callstack =
                    unsafe { backtrace.as_ref().unwrap().get_ivar("_callstack") };
                let timestamp = callstack.context.t_begin / 1000000000.0;
                let thread_index = callstack.context.thread;
                let thread_state = callstack.context.run_state;
                let frame_count = callstack.length;
                let mut frames: Vec<_> = (0..frame_count)
                    .map(|i| unsafe { *callstack.frames.offset(i as isize) })
                    .collect();
                frames.reverse();
                samples.push(Sample {
                    timestamp,
                    thread_index,
                    thread_state,
                    frames,
                });
            }
        });
        samples
    }
}

// struct {
//     struct {
//         double t_begin;
//         double t_end;
//         int pid;
//         unsigned int thread;
//         int run_state;
//         unsigned long long dispatch_queue_serial_num;
//     } context;
//     unsigned long long *frames;
//     unsigned long long *framePtrs;
//     unsigned int length;
// }  _callstack;
#[repr(C)]
#[derive(Debug)]
struct Callstack {
    context: CallstackContext,
    frames: *mut libc::c_ulonglong,
    frame_ptrs: *mut libc::c_ulonglong,
    length: libc::c_uint,
}

#[repr(C)]
#[derive(Debug)]
struct CallstackContext {
    t_begin: libc::c_double, // In nanoseconds since sampling start
    t_end: libc::c_double,   // In nanoseconds since sampling start
    pid: libc::c_int,
    thread: libc::c_uint,
    run_state: libc::c_int,
    dispatch_queue_serial_num: libc::c_ulonglong,
}

unsafe impl objc::Encode for Callstack {
    fn encode() -> objc::Encoding {
        unsafe {
            // I got this encoding by following these steps:
            //  1. Open the Symbolication binary in Hopper.
            //  2. Look up the _callstacks ivar symbol.
            //  3. There's a list of references to that symbol, double click the
            //     last reference (which is an address without a name)
            //  4. This brings you to the "struct __objc_ivar" for the symbol,
            //     which points to an aContexttbegind string for the type.
            //     That string is the one we need.
            objc::Encoding::from_str(
                r#"{?="context"{?="t_begin"d"t_end"d"pid"i"thread"I"run_state"i"dispatch_queue_serial_num"Q}"frames"^Q"framePtrs"^Q"length"I}"#,
            )
        }
    }
}
