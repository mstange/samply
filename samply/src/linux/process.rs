use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::os::fd::OwnedFd;
use std::os::raw::c_char;
use std::os::unix::prelude::OsStrExt;

use libc::{execvp, execvpe};
use nix::unistd::Pid;

/// Allows launching a command in a suspended state, so that we can know its
/// pid and initialize profiling before proceeding to execute the command.
pub struct SuspendedLaunchedProcess {
    pid: Pid,
    send_end_of_resume_pipe: OwnedFd,
    recv_end_of_execerr_pipe: OwnedFd,
}

impl SuspendedLaunchedProcess {
    pub fn launch_in_suspended_state(
        command_name: &OsStr,
        command_args: &[OsString],
        env_vars: &[(OsString, OsString)],
    ) -> std::io::Result<Self> {
        let argv: Vec<CString> = std::iter::once(command_name)
            .chain(command_args.iter().map(|s| s.as_os_str()))
            .map(|os_str: &OsStr| CString::new(os_str.as_bytes().to_vec()).unwrap())
            .collect();
        let argv: Vec<*const c_char> = argv
            .iter()
            .map(|c_str| c_str.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let envp = if !env_vars.is_empty() {
            let mut vars_os: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            for (name, val) in env_vars {
                vars_os.insert(name.to_owned(), val.to_owned());
            }
            let mut saw_nul = false;
            let envp = construct_envp(vars_os, &mut saw_nul);

            if saw_nul {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "nul byte found in environment variables",
                ));
            }
            Some(envp)
        } else {
            None
        };

        let (resume_rp, resume_sp) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();
        let (execerr_rp, execerr_sp) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();

        match unsafe { nix::unistd::fork() }.expect("Fork failed") {
            nix::unistd::ForkResult::Child => {
                // std::panic::always_abort();
                drop(resume_sp);
                drop(execerr_rp);
                Self::run_child(resume_rp, execerr_sp, &argv, envp)
            }
            nix::unistd::ForkResult::Parent { child } => {
                drop(resume_rp);
                drop(execerr_sp);
                Ok(Self {
                    pid: child,
                    send_end_of_resume_pipe: resume_sp,
                    recv_end_of_execerr_pipe: execerr_rp,
                })
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid.as_raw() as u32
    }

    const EXECERR_MSG_FOOTER: [u8; 4] = *b"NOEX";

    pub fn unsuspend_and_run(self) -> std::io::Result<RunningProcess> {
        // Send a byte to the child process.
        nix::unistd::write(&self.send_end_of_resume_pipe, &[0x42])?;
        drop(self.send_end_of_resume_pipe);

        // Wait for the child to indicate success or failure of the execve call.
        // loop for EINTR
        loop {
            let mut bytes = [0; 8];
            let read_result = nix::unistd::read(&self.recv_end_of_execerr_pipe, &mut bytes);

            // The parent has replied! Or exited.
            match read_result {
                Ok(0) => {
                    // The child closed the pipe.
                    // This means that execution was successful.
                    break;
                }
                Ok(8) => {
                    // We got an execerr message from the child. This means that the execve call failed.
                    // Decode the message.
                    let (errno, footer) = bytes.split_at(4);
                    assert_eq!(
                        Self::EXECERR_MSG_FOOTER,
                        footer,
                        "Validation on the execerr pipe failed: {bytes:?}",
                    );
                    let errno = i32::from_be_bytes([errno[0], errno[1], errno[2], errno[3]]);
                    let _wait_status = nix::sys::wait::waitpid(self.pid, None);
                    return Err(std::io::Error::from_raw_os_error(errno));
                }
                Ok(_) => {
                    // We got a message that was shorter or longer than the expected 8 bytes.
                    // It should never be shorter than 8 bytes because pipe I/O up to PIPE_BUF bytes
                    // should be atomic.

                    // This case is very unexpected and we will panic, after making sure that the child has
                    // fully executed.
                    let _status = nix::sys::wait::waitpid(self.pid, None)
                        .expect("waitpid should always succeed");

                    panic!("short read on the execerr pipe")
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(_) => std::process::exit(1),
            }
        }

        Ok(RunningProcess { pid: self.pid })
    }

    /// Executed in the forked child process. This function never returns.
    fn run_child(
        recv_end_of_resume_pipe: OwnedFd,
        send_end_of_execerr_pipe: OwnedFd,
        argv: &[*const c_char],
        envp: Option<CStringArray>,
    ) -> ! {
        // Wait for the parent to send us a byte through the pipe.
        // This will signal us to start executing.

        // loop to handle EINTR
        loop {
            let mut buf = [0];
            let read_result = nix::unistd::read(&recv_end_of_resume_pipe, &mut buf);

            // The parent has replied! Or exited.
            match read_result {
                Ok(0) => {
                    // The parent closed the pipe without telling us to start.
                    // This usually means that it encountered a problem when it tried to start
                    // profiling; in that case it just terminates, causing the pipe to close.
                    // End this process and do not execute the to-be-launched command.
                    std::process::exit(0)
                }
                Ok(_) => {
                    // The parent signaled that we can start. Exec!
                    if let Some(envp) = envp {
                        let _ = unsafe { execvpe(argv[0], argv.as_ptr(), envp.as_ptr()) };
                    } else {
                        let _ = unsafe { execvp(argv[0], argv.as_ptr()) };
                    }

                    // If executing went well, we don't get here. In that case, `send_end_of_execerr_pipe`
                    // is now closed, and the parent will notice this and proceed.

                    // But we got here! This can happen if the command doesn't exist.
                    // Return the error number via the "execerr" pipe.
                    let errno = nix::errno::Errno::last_raw().to_be_bytes();
                    let bytes = [
                        errno[0],
                        errno[1],
                        errno[2],
                        errno[3],
                        Self::EXECERR_MSG_FOOTER[0],
                        Self::EXECERR_MSG_FOOTER[1],
                        Self::EXECERR_MSG_FOOTER[2],
                        Self::EXECERR_MSG_FOOTER[3],
                    ];
                    // Send `bytes` through the pipe.
                    // Pipe I/O up to PIPE_BUF bytes should be atomic.
                    nix::unistd::write(send_end_of_execerr_pipe, &bytes).unwrap();
                    // Terminate the child process and *don't* run `at_exit` destructors as
                    // we're being torn down regardless.
                    unsafe { libc::_exit(1) }
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(_) => std::process::exit(1),
            }
        }
    }
}

pub struct RunningProcess {
    pid: Pid,
}

impl RunningProcess {
    pub fn wait(self) -> Result<nix::sys::wait::WaitStatus, nix::errno::Errno> {
        nix::sys::wait::waitpid(self.pid, None)
    }
}

// Helper type to manage ownership of the strings within a C-style array.
pub struct CStringArray {
    items: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl CStringArray {
    pub fn with_capacity(capacity: usize) -> Self {
        let mut result = CStringArray {
            items: Vec::with_capacity(capacity),
            ptrs: Vec::with_capacity(capacity + 1),
        };
        result.ptrs.push(core::ptr::null());
        result
    }
    pub fn push(&mut self, item: CString) {
        let l = self.ptrs.len();
        self.ptrs[l - 1] = item.as_ptr();
        self.ptrs.push(core::ptr::null());
        self.items.push(item);
    }
    pub fn as_ptr(&self) -> *const *const c_char {
        self.ptrs.as_ptr()
    }
}

fn construct_envp(env: BTreeMap<OsString, OsString>, saw_nul: &mut bool) -> CStringArray {
    let mut result = CStringArray::with_capacity(env.len());
    for (mut k, v) in env {
        // Reserve additional space for '=' and null terminator
        k.reserve_exact(v.len() + 2);
        k.push("=");
        k.push(&v);

        // Add the new entry into the array
        use std::os::unix::ffi::OsStringExt;
        if let Ok(item) = CString::new(k.into_vec()) {
            result.push(item);
        } else {
            *saw_nul = true;
        }
    }

    result
}
