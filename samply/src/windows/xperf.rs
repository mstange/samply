#![allow(unused)]

use std::path::{Path, PathBuf};

use fxprof_processed_profile::SamplingInterval;

use crate::shared::recording_props::RecordingProps;

pub struct Xperf {
    xperf_path: PathBuf,
    state: XperfState,
    recording_props: RecordingProps,
}

enum XperfState {
    Stopped,
    RecordingKernelToFile(PathBuf),
    RecordingKernelAndUserToFile(PathBuf, PathBuf),
}

impl Xperf {
    pub fn new(recording_props: RecordingProps) -> Result<Self, which::Error> {
        let xperf_path = which::which("xperf")?;
        Ok(Self {
            xperf_path,
            state: XperfState::Stopped,
            recording_props,
        })
    }

    pub fn is_running(&self) -> bool {
        matches!(
            &self.state,
            XperfState::RecordingKernelToFile(_) | XperfState::RecordingKernelAndUserToFile(_, _)
        )
    }

    pub fn start_xperf(&mut self, interval: SamplingInterval) {
        if self.is_running() {
            self.stop_xperf();
        }

        let output_file = &self.recording_props.output_file;

        // All the user providers need to be specified in a single `-on` argument
        // with "+" in between.
        let mut user_providers = vec![];

        user_providers.append(&mut super::coreclr::coreclr_xperf_args(
            &self.recording_props,
        ));

        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let mut kernel_etl_file = expand_full_filename_with_cwd(output_file);
        kernel_etl_file.set_extension("unmerged-etl");

        const NANOS_PER_TICK: u64 = 100;
        let interval_ticks = interval.nanos() / NANOS_PER_TICK;

        let mut xperf = runas::Command::new(&self.xperf_path);
        xperf.arg("-SetProfInt");
        xperf.arg(interval_ticks.to_string());

        // Virtualised ARM64 Windows crashes out on PROFILE tracing, so this hidden
        // hack argument lets things still continue to run for development of samply.
        xperf.arg("-on");
        if !self.recording_props.vm_hack {
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
            let mut user_etl_file = kernel_etl_file.clone();
            user_etl_file.set_extension("user-unmerged-etl");

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
    }

    pub fn stop_xperf(&mut self) -> Option<PathBuf> {
        let prev_state = std::mem::replace(&mut self.state, XperfState::Stopped);
        let (kernel_etl, user_etl) = match prev_state {
            XperfState::Stopped => return None,
            XperfState::RecordingKernelToFile(kpath) => (kpath, None),
            XperfState::RecordingKernelAndUserToFile(kpath, upath) => (kpath, Some(upath)),
        };
        let merged_etl = kernel_etl.with_extension("etl");

        let mut xperf = runas::Command::new(&self.xperf_path);
        xperf.arg("-stop");

        if user_etl.is_some() {
            xperf.arg("-stop");
            xperf.arg("SamplySession");
        }

        xperf.arg("-d");
        xperf.arg(&merged_etl);

        let _ = xperf
            .status()
            .expect("Failed to execute xperf -stop! xperf may still be recording.");

        eprintln!("xperf session stopped.");

        std::fs::remove_file(&kernel_etl).unwrap_or_else(|_| {
            panic!(
                "Failed to delete unmerged ETL file {:?}",
                kernel_etl.to_str().unwrap()
            )
        });

        if let Some(user_etl) = &user_etl {
            std::fs::remove_file(user_etl).unwrap_or_else(|_| {
                panic!(
                    "Failed to delete unmerged ETL file {:?}",
                    user_etl.to_str().unwrap()
                )
            });
        }

        Some(merged_etl)
    }
}

impl Drop for Xperf {
    fn drop(&mut self) {
        // we should probably xperf -cancel here instead of doing the merge on drop...
        self.stop_xperf();
    }
}

fn expand_full_filename_with_cwd(filename: &Path) -> PathBuf {
    if filename.is_absolute() {
        filename.to_path_buf()
    } else {
        let mut fullpath = std::env::current_dir().unwrap();
        fullpath.push(filename);
        fullpath
    }
}
