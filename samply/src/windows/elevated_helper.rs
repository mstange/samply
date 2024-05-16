use std::error::Error;
use std::path::{Path, PathBuf};

use serde_derive::{Deserialize, Serialize};

use crate::shared::recording_props::{RecordingMode, RecordingProps};

use super::utility_process::{
    run_child, UtilityProcess, UtilityProcessChild, UtilityProcessParent, UtilityProcessSession,
};
use super::xperf::Xperf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
enum ElevatedHelperRequestMsg {
    StartXperf(ElevatedRecordingProps),
    StopXperf,
    GetKernelModules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElevatedRecordingProps {
    pub time_limit_seconds: Option<f64>,
    pub interval_nanos: u64,
    pub coreclr: bool,
    pub coreclr_allocs: bool,
    pub vm_hack: bool,
    pub is_attach: bool,
    pub gfx: bool,
}

impl ElevatedRecordingProps {
    pub fn from_recording_props(
        recording_props: &RecordingProps,
        recording_mode: &RecordingMode,
    ) -> Self {
        Self {
            time_limit_seconds: recording_props.time_limit.map(|l| l.as_secs_f64()),
            interval_nanos: recording_props.interval.as_nanos().try_into().unwrap(),
            coreclr: recording_props.coreclr,
            coreclr_allocs: recording_props.coreclr_allocs,
            vm_hack: recording_props.vm_hack,
            is_attach: recording_mode.is_attach_mode(),
            gfx: recording_props.gfx,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
#[allow(clippy::enum_variant_names)]
enum ElevatedHelperReplyMsg {
    AckStartXperf,
    AckStopXperf(PathBuf),
    AckGetKernelModules,
}

// Runs in the helper process which has Administrator privileges.
pub fn run_elevated_helper(ipc_directory: &Path, output_path: PathBuf) {
    let child = ElevatedHelperChild::new(output_path);
    run_child::<ElevatedHelper>(ipc_directory, child)
}

pub struct ElevatedHelperSession {
    elevated_session: UtilityProcessSession<ElevatedHelper>,
}

impl ElevatedHelperSession {
    pub fn new(output_path: PathBuf) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let parent = ElevatedHelperParent { output_path };
        let elevated_session = UtilityProcessSession::spawn_process(parent)?;
        Ok(Self { elevated_session })
    }

    pub fn start_xperf(
        &mut self,
        recording_props: &RecordingProps,
        recording_mode: &RecordingMode,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let xperf_args =
            ElevatedRecordingProps::from_recording_props(recording_props, recording_mode);
        match self
            .elevated_session
            .send_msg_and_wait_for_response(ElevatedHelperRequestMsg::StartXperf(xperf_args))
        {
            Ok(reply) => match reply {
                ElevatedHelperReplyMsg::AckStartXperf => Ok(()),
                other_msg => {
                    Err(format!("Unexpected reply to StartXperf msg: {other_msg:?}").into())
                }
            },
            Err(err) => {
                eprintln!("Could not start xperf: {err}");
                std::process::exit(1);
            }
        }
    }

    pub fn stop_xperf(&mut self) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        let reply = self
            .elevated_session
            .send_msg_and_wait_for_response(ElevatedHelperRequestMsg::StopXperf)?;
        match reply {
            ElevatedHelperReplyMsg::AckStopXperf(path) => Ok(path),
            other_msg => Err(format!("Unexpected reply to StartXperf msg: {other_msg:?}").into()),
        }
    }

    pub fn shutdown(self) {
        self.elevated_session.shutdown()
    }
}

struct ElevatedHelper;

impl UtilityProcess for ElevatedHelper {
    const PROCESS_TYPE: &'static str = "windows-elevated-helper";
    type Child = ElevatedHelperChild;
    type Parent = ElevatedHelperParent;
    type ParentToChildMsg = ElevatedHelperRequestMsg;
    type ChildToParentMsg = ElevatedHelperReplyMsg;
}

struct ElevatedHelperParent {
    output_path: PathBuf,
}

impl UtilityProcessParent for ElevatedHelperParent {
    fn spawn_child(self, ipc_directory: &Path) {
        let self_path = std::env::current_exe().expect("Couldn't obtain path of this binary");
        // eprintln!(
        //     "Run this: {} run-elevated-helper --ipc-directory {} --output-path {}",
        //     self_path.to_string_lossy(),
        //     ipc_directory.to_string_lossy(),
        //     self.output_path.to_string_lossy()
        // );

        // let mut cmd = std::process::Command::new(&self_path);
        let mut cmd = runas::Command::new(self_path);
        cmd.arg("run-elevated-helper");

        cmd.arg("--ipc-directory");
        cmd.arg(ipc_directory);
        cmd.arg("--output-path");
        cmd.arg(expand_full_filename_with_cwd(&self.output_path));

        let _ = cmd.status().expect("Failed to execute elevated helper");
    }
}

pub fn expand_full_filename_with_cwd(filename: &Path) -> PathBuf {
    if filename.is_absolute() {
        filename.to_path_buf()
    } else {
        let mut fullpath = std::env::current_dir().unwrap();
        fullpath.push(filename);
        fullpath
    }
}

struct ElevatedHelperChild {
    output_path: PathBuf,
    xperf: Xperf,
}

impl ElevatedHelperChild {
    // Runs in the helper process which has Administrator privileges.
    pub fn new(output_path: PathBuf) -> Self {
        Self {
            xperf: Xperf::new(),
            output_path,
        }
    }
}

impl UtilityProcessChild<ElevatedHelperRequestMsg, ElevatedHelperReplyMsg> for ElevatedHelperChild {
    // Runs in the helper process which has Administrator privileges.
    fn handle_message(
        &mut self,
        msg: ElevatedHelperRequestMsg,
    ) -> Result<ElevatedHelperReplyMsg, Box<dyn Error + Send + Sync>> {
        match msg {
            ElevatedHelperRequestMsg::StartXperf(props) => {
                self.xperf.start_xperf(&self.output_path, &props)?;
                Ok(ElevatedHelperReplyMsg::AckStartXperf)
            }
            ElevatedHelperRequestMsg::StopXperf => {
                let output_file = self.xperf.stop_xperf()?;
                Ok(ElevatedHelperReplyMsg::AckStopXperf(output_file))
            }
            ElevatedHelperRequestMsg::GetKernelModules => Err("todo".into()),
        }
    }
}
