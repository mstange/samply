use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Write;
use std::os::unix::prelude::OsStrExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

use flate2::write::GzDecoder;
use mach::task::{task_resume, task_suspend};
use mach::traps::task_for_pid;
use tempfile::tempdir;

use crate::shared::ctrl_c::CtrlC;

pub use super::mach_ipc::{mach_port_t, MachError, OsIpcSender};
use super::mach_ipc::{mach_task_self, BlockingMode, OsIpcMultiShotServer, MACH_PORT_NULL};

pub trait RootTaskRunner {
    fn run_root_task(&self) -> Result<ExitStatus, MachError>;
}

pub struct TaskLauncher {
    program: OsString,
    args: Vec<OsString>,
    child_env: Vec<(OsString, OsString)>,
    iteration_count: u32,
}

impl RootTaskRunner for TaskLauncher {
    fn run_root_task(&self) -> Result<ExitStatus, MachError> {
        // Ignore Ctrl+C while the subcommand is running. The signal still reaches the process
        // under observation while we continue to record it. (ctrl+c will send the SIGINT signal
        // to all processes in the foreground process group).
        let mut ctrl_c_receiver = CtrlC::observe_oneshot();

        let mut root_child = self.launch_child();
        let mut exit_status = root_child.wait().expect("couldn't wait for child");

        for i in 2..=self.iteration_count {
            if !exit_status.success() {
                eprintln!(
                    "Skipping remaining iterations due to non-success exit status: \"{}\"",
                    exit_status
                );
                break;
            }
            eprintln!("Running iteration {i} of {}...", self.iteration_count);
            let mut root_child = self.launch_child();
            exit_status = root_child.wait().expect("couldn't wait for child");
        }

        // From now on, we want to terminate if the user presses Ctrl+C.
        ctrl_c_receiver.close();

        Ok(exit_status)
    }
}

impl TaskLauncher {
    pub fn new<I, S>(
        program: S,
        args: I,
        iteration_count: u32,
        env_vars: &[(OsString, OsString)],
        extra_env_vars: &[(OsString, OsString)],
    ) -> Result<TaskLauncher, MachError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        // Take this process's environment variables and add DYLD_INSERT_LIBRARIES
        // and SAMPLY_BOOTSTRAP_SERVER_NAME.
        let mut child_env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        for (name, val) in env_vars {
            child_env.insert(name.to_owned(), val.to_owned());
        }
        for (name, val) in extra_env_vars {
            child_env.insert(name.to_owned(), val.to_owned());
        }
        let child_env: Vec<(OsString, OsString)> = child_env.into_iter().collect();

        let args: Vec<OsString> = args.into_iter().map(|a| a.into()).collect();
        let program: OsString = program.into();

        Ok(TaskLauncher {
            program,
            args,
            child_env,
            iteration_count,
        })
    }

    pub fn launch_child(&self) -> Child {
        match Command::new(&self.program)
            .args(&self.args)
            .envs(self.child_env.clone())
            .spawn()
        {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let command_name = self.program.to_string_lossy();
                if command_name.starts_with('-') {
                    eprintln!("error: unexpected argument '{command_name}' found");
                } else {
                    eprintln!("Error: Could not find an executable with the name {command_name}.");
                }
                std::process::exit(1)
            }
            Err(err) => {
                eprintln!("Error: Could not launch child process: {err}");
                std::process::exit(1)
            }
        }
    }
}

pub struct TaskAccepter {
    server: OsIpcMultiShotServer,
    added_env: Vec<(OsString, OsString)>,
    queue: Vec<ReceivedStuff>,
    _temp_dir: Arc<tempfile::TempDir>,
}

static PRELOAD_LIB_CONTENTS: &[u8] =
    include_bytes!("../../resources/libsamply_mac_preload.dylib.gz");

impl TaskAccepter {
    pub fn new() -> Result<Self, MachError> {
        let (server, server_name) = OsIpcMultiShotServer::new()?;

        // Launch the child with DYLD_INSERT_LIBRARIES set to libsamply_mac_preload.dylib.

        // We would like to ship with libsamply_mac_preload.dylib as a separate resource file.
        // But this won't work with cargo install. So we write out libsamply_mac_preload.dylib
        // to a temporary directory.
        let dir = tempdir().expect("Couldn't create temporary directory for preload-lib");
        let preload_lib_path = dir.path().join("libsamply_mac_preload.dylib");
        let file =
            File::create(&preload_lib_path).expect("Couldn't create libsamply_mac_preload.dylib");
        let mut decoder = GzDecoder::new(file);
        decoder
            .write_all(PRELOAD_LIB_CONTENTS)
            .expect("Couldn't write libsamply_mac_preload.dylib (error during write_all)");
        decoder
            .finish()
            .expect("Couldn't write libsamply_mac_preload.dylib (error during finish)");

        let mut added_env: Vec<(OsString, OsString)> = vec![];
        let mut add_env = |name: &str, val: &OsStr| {
            added_env.push((name.into(), val.to_owned()));
            // Also set the same variable with an `__XPC_` prefix, so that it gets applied
            // to services launched via XPC. XPC strips the prefix when setting these environment
            // variables on the launched process.
            added_env.push((format!("__XPC_{name}").into(), val.to_owned()));
        };
        add_env("DYLD_INSERT_LIBRARIES", preload_lib_path.as_os_str());
        add_env("SAMPLY_BOOTSTRAP_SERVER_NAME", OsStr::new(&server_name));

        Ok(TaskAccepter {
            server,
            added_env,
            queue: vec![],
            _temp_dir: Arc::new(dir),
        })
    }

    pub fn extra_env_vars(&self) -> &[(OsString, OsString)] {
        &self.added_env
    }

    pub fn queue_received_stuff(&mut self, rs: ReceivedStuff) {
        self.queue.push(rs);
    }

    pub fn next_message(&mut self, timeout: Duration) -> Result<ReceivedStuff, MachError> {
        if let Some(rs) = self.queue.pop() {
            return Ok(rs);
        }

        // Wait until the child is ready
        let (res, mut channels, _) = self
            .server
            .accept(BlockingMode::BlockingWithTimeout(timeout))?;
        let received_stuff = match res.split_at(7) {
            (b"My task", pid_bytes) => {
                assert!(pid_bytes.len() == 4);
                let pid =
                    u32::from_le_bytes([pid_bytes[0], pid_bytes[1], pid_bytes[2], pid_bytes[3]]);
                let task_channel = channels.pop().unwrap();
                let sender_channel = channels.pop().unwrap();
                let sender_channel = sender_channel.into_sender();

                let task = task_channel.into_port();

                ReceivedStuff::AcceptedTask(AcceptedTask {
                    task,
                    pid,
                    sender_channel: Some(sender_channel),
                })
            }
            (b"Jitdump", jitdump_info) => {
                let pid_bytes = &jitdump_info[0..4];
                let pid =
                    u32::from_le_bytes([pid_bytes[0], pid_bytes[1], pid_bytes[2], pid_bytes[3]]);
                let len = jitdump_info[4] as usize;
                let path = &jitdump_info[5..][..len];
                ReceivedStuff::JitdumpPath(pid, OsStr::from_bytes(path).into())
            }
            (b"MarkerF", marker_file_info) => {
                let pid_bytes = &marker_file_info[0..4];
                let pid =
                    u32::from_le_bytes([pid_bytes[0], pid_bytes[1], pid_bytes[2], pid_bytes[3]]);
                let len = marker_file_info[4] as usize;
                let path = &marker_file_info[5..][..len];
                ReceivedStuff::MarkerFilePath(pid, OsStr::from_bytes(path).into())
            }
            (other, _) => {
                panic!("Unexpected message: {:?}", other);
            }
        };
        Ok(received_stuff)
    }
}

pub enum ReceivedStuff {
    AcceptedTask(AcceptedTask),
    JitdumpPath(u32, PathBuf),
    MarkerFilePath(u32, PathBuf),
}

pub struct AcceptedTask {
    task: mach_port_t,
    pid: u32,
    sender_channel: Option<OsIpcSender>,
}

impl AcceptedTask {
    pub fn task(&self) -> mach_port_t {
        self.task
    }

    pub fn get_id(&self) -> u32 {
        self.pid
    }

    pub fn start_execution(&self) {
        if let Some(sender_channel) = &self.sender_channel {
            sender_channel.send(b"Proceed", vec![]).unwrap();
        } else {
            unsafe { task_resume(self.task) };
        }
    }
}

pub struct ExistingProcessRunner {
    pid: u32,
}

impl RootTaskRunner for ExistingProcessRunner {
    fn run_root_task(&self) -> Result<ExitStatus, MachError> {
        let ctrl_c_receiver = CtrlC::observe_oneshot();

        eprintln!("Profiling {}, press Ctrl-C to stop...", self.pid);

        ctrl_c_receiver
            .blocking_recv()
            .expect("Ctrl+C receiver failed");

        eprintln!("Done.");

        Ok(ExitStatus::default())
    }
}

impl ExistingProcessRunner {
    pub fn new(pid: u32, task_accepter: &mut TaskAccepter) -> ExistingProcessRunner {
        let mut queue_pid = |pid, failure_is_ok| {
            let task = unsafe {
                let mut task = MACH_PORT_NULL;
                let kr = task_for_pid(mach_task_self(), pid as i32, &mut task);
                if kr != 0 {
                    if failure_is_ok {
                        eprintln!("Warning: task_for_pid for child task failed with error code {kr}. Ignoring child, it may have already exited.");
                        return;
                    }

                    eprintln!("Error: task_for_pid for target task failed with error code {kr}. Does the profiler have entitlements?");
                    std::process::exit(1);
                }
                task_suspend(task);
                task
            };
            task_accepter.queue_received_stuff(ReceivedStuff::AcceptedTask(AcceptedTask {
                task,
                pid,
                sender_channel: None,
            }));
        };

        // always root pid first
        queue_pid(pid, true);

        // TODO: find all its children

        ExistingProcessRunner { pid }
    }
}
