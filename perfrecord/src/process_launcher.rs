use std::fs::File;
use std::io::Write;
use std::mem;
use std::process::{Child, Command};
use std::time::Duration;

pub use perfrecord_mach_ipc_rendezvous::{mach_port_t, MachError, OsIpcSender};
use perfrecord_mach_ipc_rendezvous::{BlockingMode, OsIpcMultiShotServer, MACH_PORT_NULL};
use tempfile::tempdir;

pub struct TaskAccepter {
    server: OsIpcMultiShotServer,
    _temp_dir: tempfile::TempDir,
}

static PRELOAD_LIB_CONTENTS: &'static [u8] =
    include_bytes!("../resources/libperfrecord_preload.dylib");

impl TaskAccepter {
    pub fn create_and_launch_root_task(
        program: &str,
        args: &[&str],
    ) -> Result<(Self, Child), MachError> {
        let (server, server_name) = OsIpcMultiShotServer::new()?;

        // Launch the child with DYLD_INSERT_LIBRARIES set to libperfrecord_preload.dylib.

        // We would like to ship with libperfrecord_preload.dylib as a separate resource file.
        // But this won't work with cargo install. So we write out libperfrecord_preload.dylib
        // to a temporary directory.
        let dir = tempdir().expect("Couldn't create temporary directory for preload-lib");
        let preload_lib_path = dir.path().join("libperfrecord_preload.dylib");
        let mut file =
            File::create(&preload_lib_path).expect("Couldn't create libperfrecord_preload.dylib");
        file.write_all(PRELOAD_LIB_CONTENTS)
            .expect("Couldn't write libperfrecord_preload.dylib");
        mem::drop(file);

        // Take this process's environment variables and add DYLD_INSERT_LIBRARIES
        // and PERFRECORD_BOOTSTRAP_SERVER_NAME.
        let child_env = std::env::vars_os()
            .chain(std::iter::once((
                "DYLD_INSERT_LIBRARIES".into(),
                preload_lib_path.into(),
            )))
            .chain(std::iter::once((
                "PERFRECORD_BOOTSTRAP_SERVER_NAME".into(),
                server_name.into(),
            )));

        let root_child = Command::new(program)
            .args(args)
            .envs(child_env)
            .spawn()
            .expect("launching child unsuccessful");

        Ok((
            TaskAccepter {
                server,
                _temp_dir: dir,
            },
            root_child,
        ))
    }

    pub fn try_accept(&mut self, timeout: Duration) -> Result<AcceptedTask, MachError> {
        // Wait until the child is ready
        let (res, mut channels, _) = self
            .server
            .accept(BlockingMode::BlockingWithTimeout(timeout))?;
        assert_eq!(res.len(), 11);
        assert!(&res[0..7] == b"My task");
        let mut pid_bytes: [u8; 4] = Default::default();
        pid_bytes.copy_from_slice(&res[7..11]);
        let pid = u32::from_le_bytes(pid_bytes);
        let mut task_channel = channels.pop().unwrap();
        let mut sender_channel = channels.pop().unwrap();
        let sender_channel = sender_channel.to_sender();

        let task = task_channel.to_port();

        Ok(AcceptedTask {
            task,
            pid,
            sender_channel,
        })
    }
}

pub struct AcceptedTask {
    task: mach_port_t,
    pid: u32,
    sender_channel: OsIpcSender,
}

impl AcceptedTask {
    pub fn take_task(&mut self) -> mach_port_t {
        mem::replace(&mut self.task, MACH_PORT_NULL)
    }

    pub fn get_id(&self) -> u32 {
        self.pid
    }

    pub fn start_execution(&self) {
        self.sender_channel
            .send(b"Proceed", vec![], vec![])
            .unwrap();
    }
}
