use perfrecord_mach_ipc_rendezvous::{OsIpcOneShotServer, OsIpcSender};
use tempfile::tempdir;

use std::fs::File;
use std::io::{self, Write};
use std::mem;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};

pub use perfrecord_mach_ipc_rendezvous::{mach_port_t, MachError, MACH_PORT_NULL};

pub struct ProcessLauncher {
    child_task: mach_port_t,
    child: Child,
    sender_channel: OsIpcSender,
    _temp_dir: tempfile::TempDir,
}

static PRELOAD_LIB_CONTENTS: &'static [u8] =
    include_bytes!("../resources/libperfrecord_preload.dylib");

impl ProcessLauncher {
    pub fn new(binary: &Path, args: &[&str]) -> Result<Self, MachError> {
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

        let (server, server_name) = OsIpcOneShotServer::new()?;

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

        let child = Command::new(binary)
            .args(args)
            .envs(child_env)
            .spawn()
            .expect("launching child unsuccessful");

        // Wait until the child is ready
        let (_server, res, mut channels, _) = server.accept()?;
        assert!(res == b"My task");
        let mut task_channel = channels.pop().unwrap();
        let mut sender_channel = channels.pop().unwrap();
        let sender_channel = sender_channel.to_sender();

        let child_task = task_channel.to_port();

        Ok(ProcessLauncher {
            child_task,
            child,
            sender_channel,
            _temp_dir: dir,
        })
    }

    pub fn take_task(&mut self) -> mach_port_t {
        mem::replace(&mut self.child_task, MACH_PORT_NULL)
    }

    pub fn get_id(&self) -> u32 {
        self.child.id()
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    pub fn start_execution(&self) {
        self.sender_channel
            .send(b"Proceed", vec![], vec![])
            .unwrap();
    }
}
