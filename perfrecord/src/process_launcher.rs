
use std::ffi::CString;
use mach_ipc_rendezvous::{OsIpcOneShotServer, OsIpcSender};
use libc;
use std::mem;
use std::ptr;

pub use mach_ipc_rendezvous::{MachError, mach_port_t, MACH_PORT_NULL};

pub struct ProcessLauncher {
    child_task: mach_port_t,
    child_pid: libc::pid_t,
    sender_channel: OsIpcSender,
}

static PRELOAD_LIB_PATH: &'static str = "/Users/mstange/code/perfrecord/perfrecord-preload/target/release/libperfrecord_preload.dylib";

impl ProcessLauncher {
    pub fn new(
        binary: &str,
        argv: &[&str],
        env: &[&str],
        working_dir: &str,
    ) -> Result<Self, MachError> {
        let (server, name) = OsIpcOneShotServer::new()?;

        let mut child_env: Vec<CString> = env
            .iter()
            .map(|env_var| CString::new(env_var.as_bytes()).unwrap())
            .collect();
        child_env
            .push(CString::new(format!("DYLD_INSERT_LIBRARIES={}", PRELOAD_LIB_PATH)).unwrap());
        child_env.push(CString::new(format!("PERFRECORD_BOOTSTRAP_SERVER_NAME={}", name)).unwrap());

        let mut child_env: Vec<_> = child_env.iter().map(|e| e.as_ptr()).collect();
        child_env.push(std::ptr::null());

        let child_args: Vec<_> = argv.iter().map(|a| CString::new(*a).unwrap()).collect();
        let mut child_args: Vec<_> = child_args.iter().map(|a| a.as_ptr()).collect();
        child_args.push(std::ptr::null());

        let child_pid = unsafe {
            fork(|| {
                libc::chdir(CString::new(working_dir).unwrap().as_ptr());
                libc::execve(
                    CString::new(binary).unwrap().as_ptr(),
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
