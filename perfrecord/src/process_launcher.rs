use libc;
use mach_ipc_rendezvous::{OsIpcOneShotServer, OsIpcSender};
use tempfile::tempdir;

use std::ffi::CString;
use std::mem;
use std::path::Path;
use std::ptr;
use std::fs::File;
use std::io::Write;

pub use mach_ipc_rendezvous::{mach_port_t, MachError, MACH_PORT_NULL};

pub struct ProcessLauncher {
    child_task: mach_port_t,
    child_pid: libc::pid_t,
    sender_channel: OsIpcSender,
}

static PRELOAD_LIB_CONTENTS: &'static [u8] = include_bytes!("../resources/libperfrecord_preload.dylib");

impl ProcessLauncher {
    pub fn new(binary: &Path, argv: &[&str], env: &[&str]) -> Result<Self, MachError> {
        // Launch the child with DYLD_INSERT_LIBRARIES set to libperfrecord_preload.dylib.

        // We would like to ship with libperfrecord_preload.dylib as a separate resource file.
        // But this won't work with cargo install. So we write out libperfrecord_preload.dylib
        // to a temporary directory.
        let dir = tempdir().expect("Couldn't create temporary directory for preload-lib");
        let preload_lib_path = dir.path().join("libperfrecord_preload.dylib");
        let mut file = File::create(&preload_lib_path).expect("Couldn't create libperfrecord_preload.dylib");
        file.write_all(PRELOAD_LIB_CONTENTS).expect("Couldn't write libperfrecord_preload.dylib");
        mem::drop(file);
        let preload_lib_path = preload_lib_path.as_os_str().to_str().expect("Couldn't convert path to string");

        let (server, name) = OsIpcOneShotServer::new()?;

        let mut child_env: Vec<CString> = env
            .iter()
            .map(|env_var| CString::new(env_var.as_bytes()).unwrap())
            .collect();
        child_env
            .push(CString::new(format!("DYLD_INSERT_LIBRARIES={}", preload_lib_path)).unwrap());
        child_env.push(CString::new(format!("PERFRECORD_BOOTSTRAP_SERVER_NAME={}", name)).unwrap());

        let mut child_env: Vec<_> = child_env.iter().map(|e| e.as_ptr()).collect();
        child_env.push(std::ptr::null());

        let child_args: Vec<_> = argv.iter().map(|a| CString::new(*a).unwrap()).collect();
        let mut child_args: Vec<_> = child_args.iter().map(|a| a.as_ptr()).collect();
        child_args.push(std::ptr::null());

        let child_pid = unsafe {
            fork(|| {
                use std::os::unix::ffi::OsStrExt;
                libc::execve(
                    CString::new(binary.as_os_str().as_bytes())
                        .unwrap()
                        .as_ptr(),
                    child_args.as_ptr(),
                    child_env.as_ptr(),
                );
            })
        };
        // Wait until the child is ready
        let (_server, res, mut channels, _) = server.accept()?;
        assert!(res == b"My task");
        let mut task_channel = channels.pop().unwrap();
        let mut sender_channel = channels.pop().unwrap();
        let sender_channel = sender_channel.to_sender();

        let child_task = task_channel.to_port();

        Ok(ProcessLauncher {
            child_task,
            child_pid,
            sender_channel,
        })
    }

    pub fn take_task(&mut self) -> mach_port_t {
        mem::replace(&mut self.child_task, MACH_PORT_NULL)
    }

    pub fn get_pid(&self) -> libc::pid_t {
        self.child_pid
    }

    pub fn start_execution(&self) {
        self.sender_channel
            .send(b"Proceed", vec![], vec![])
            .unwrap();
    }
}

// I'm not actually sure invoking this is indeed unsafe -- but better safe than sorry...
unsafe fn fork<F: FnOnce()>(child_func: F) -> libc::pid_t {
    match libc::fork() {
        -1 => panic!("Fork failed: {}", std::io::Error::last_os_error()),
        0 => {
            child_func();
            libc::exit(0);
        }
        pid => pid,
    }
}

trait Wait {
    fn wait(self);
}

impl Wait for libc::pid_t {
    fn wait(self) {
        unsafe {
            libc::waitpid(self, ptr::null_mut(), 0);
        }
    }
}
