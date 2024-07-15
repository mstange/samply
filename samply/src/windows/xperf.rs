use std::error::Error;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::elevated_helper::ElevatedRecordingProps;

const XPERF_NOT_FOUND_ERROR_MSG: &str = "\
Could not find an xperf installation.\n\
Please install the Windows Performance Toolkit: https://learn.microsoft.com/en-us/windows-hardware/test/wpt/
(Download the ADK from https://go.microsoft.com/fwlink/?linkid=2243390 and uncheck everything
except \"Windows Performance Toolkit\" during the installation.)";

pub struct Xperf {
    state: XperfState,
    xperf_path: Option<PathBuf>,
}

enum XperfState {
    Stopped,
    RecordingKernelToFile(PathBuf),
    RecordingKernelAndUserToFile(PathBuf, PathBuf),
}

impl Xperf {
    pub fn new() -> Self {
        Self {
            state: XperfState::Stopped,
            xperf_path: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(
            &self.state,
            XperfState::RecordingKernelToFile(_) | XperfState::RecordingKernelAndUserToFile(_, _)
        )
    }

    fn get_xperf_path(&mut self) -> Result<PathBuf, &'static str> {
        if let Some(p) = self.xperf_path.clone() {
            return Ok(p);
        }
        let xperf_path = which::which("xperf").map_err(|_| XPERF_NOT_FOUND_ERROR_MSG)?;
        self.xperf_path = Some(xperf_path.clone());
        Ok(xperf_path)
    }

    pub fn start_xperf(
        &mut self,
        output_path: &Path,
        props: &ElevatedRecordingProps,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.is_running() {
            let _ = self.stop_xperf();
        }

        // All the user providers need to be specified in a single `-on` argument
        // with "+" in between.
        let mut user_providers = vec![];

        user_providers.append(&mut super::coreclr::coreclr_xperf_args(props));
        user_providers.append(&mut super::gfx::gfx_xperf_args(props));
        user_providers.append(&mut super::firefox::firefox_xperf_args(props));
        user_providers.append(&mut super::chrome::chrome_xperf_args(props));
        user_providers.sort_unstable();
        user_providers.dedup();

        let xperf_path = self.get_xperf_path()?;
        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let mut kernel_etl_file = output_path.to_owned();
        if kernel_etl_file.extension() == Some(OsStr::new("gz")) {
            kernel_etl_file.set_extension("");
        }
        kernel_etl_file.set_extension("kernel.etl");

        const MIN_INTERVAL_NANOS: u64 = 122100; // 8192 kHz
        let interval_nanos = props.interval_nanos.clamp(MIN_INTERVAL_NANOS, u64::MAX);
        const NANOS_PER_TICK: u64 = 100;
        let interval_ticks = interval_nanos / NANOS_PER_TICK;

        let mut xperf = std::process::Command::new(xperf_path);
        xperf.arg("-SetProfInt");
        xperf.arg(interval_ticks.to_string());

        // Virtualised ARM64 Windows crashes out on PROFILE tracing, so this hidden
        // hack argument lets things still continue to run for development of samply.
        xperf.arg("-on");
        if !props.vm_hack {
            xperf.arg("PROC_THREAD+LOADER+PROFILE+CSWITCH");
            xperf.arg("-stackwalk");
            xperf.arg("PROFILE+CSWITCH");
        } else {
            // virtualized arm64 hack, to give us enough interesting events
            xperf.arg("PROC_THREAD+LOADER+CSWITCH+SYSCALL+VIRT_ALLOC+OB_HANDLE");
            xperf.arg("-stackwalk");
            xperf.arg("CSWITCH+VirtualAlloc+VirtualFree+HandleCreate+HandleClose");
        }
        xperf.arg("-f");
        xperf.arg(&kernel_etl_file);

        let user_etl_file = if !user_providers.is_empty() {
            let mut user_etl_file = output_path.to_owned();
            if user_etl_file.extension() == Some(OsStr::new("gz")) {
                user_etl_file.set_extension("");
            }
            user_etl_file.set_extension("user.etl");

            xperf.arg("-start");
            xperf.arg("SamplySession");

            xperf.arg("-on");
            xperf.arg(user_providers.join("+"));

            xperf.arg("-f");
            xperf.arg(&user_etl_file);

            Some(user_etl_file)
        } else {
            None
        };

        let _ = xperf.status().expect("failed to execute xperf");

        eprintln!("xperf session running...");

        if user_etl_file.is_some() {
            self.state =
                XperfState::RecordingKernelAndUserToFile(kernel_etl_file, user_etl_file.unwrap());
        } else {
            self.state = XperfState::RecordingKernelToFile(kernel_etl_file);
        }

        Ok(())
    }

    pub fn stop_xperf(
        &mut self,
    ) -> Result<(PathBuf, Option<PathBuf>), Box<dyn Error + Send + Sync>> {
        let prev_state = std::mem::replace(&mut self.state, XperfState::Stopped);
        let (kernel_etl, user_etl) = match prev_state {
            XperfState::Stopped => return Err("xperf wasn't running, can't stop it".into()),
            XperfState::RecordingKernelToFile(kpath) => (kpath, None),
            XperfState::RecordingKernelAndUserToFile(kpath, upath) => (kpath, Some(upath)),
        };

        let xperf_path = self.get_xperf_path()?;
        let mut xperf = std::process::Command::new(xperf_path);
        xperf.arg("-stop");

        if user_etl.is_some() {
            xperf.arg("-stop");
            xperf.arg("SamplySession");
        }

        let _ = xperf
            .status()
            .expect("Failed to execute xperf -stop! xperf may still be recording.");

        eprintln!("xperf session stopped.");

        Ok((kernel_etl, user_etl))
    }
}

impl Drop for Xperf {
    fn drop(&mut self) {
        // we should probably xperf -cancel here instead of doing the merge on drop...
        let _ = self.stop_xperf();
    }
}
