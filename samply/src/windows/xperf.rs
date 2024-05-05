#![allow(unused)]

use std::path::{Path, PathBuf};

pub struct Xperf {
    arch: String,
    xperf_path: PathBuf,
    state: XperfState,
    capture_coreclr: bool,
    virtualized_aarch64_hack: bool,
}

enum XperfState {
    Stopped,
    RecordingKernelToFile(PathBuf),
    RecordingKernelAndUserToFile(PathBuf, PathBuf),
}

impl Xperf {
    pub fn new(arch: String) -> Result<Self, which::Error> {
        let xperf_path = which::which("xperf")?;
        Ok(Self {
            xperf_path,
            arch,
            state: XperfState::Stopped,
            capture_coreclr: false,
            virtualized_aarch64_hack: false,
        })
    }

    // TODO turn this into a generic mechanism to add additional providers
    pub fn set_capture_coreclr(&mut self, capture_coreclr: bool) {
        self.capture_coreclr = capture_coreclr;
    }

    pub fn set_virtualized_aarch64_hack(&mut self, virtualized_aarch64_hack: bool) {
        self.virtualized_aarch64_hack = virtualized_aarch64_hack;
    }

    pub fn is_running(&self) -> bool {
        matches!(
            &self.state,
            XperfState::RecordingKernelToFile(_) | XperfState::RecordingKernelAndUserToFile(_, _)
        )
    }

    pub fn start_xperf(&mut self, output_file: &Path) {
        if self.is_running() {
            self.stop_xperf();
        }

        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let mut kernel_etl_file = expand_full_filename_with_cwd(output_file);
        kernel_etl_file.set_extension("unmerged-etl");

        let user_etl_file = if self.capture_coreclr {
            let mut user_etl_file = kernel_etl_file.clone();
            user_etl_file.set_extension("user-unmerged-etl");
            Some(user_etl_file)
        } else {
            None
        };

        let mut xperf = runas::Command::new(&self.xperf_path);

        xperf.arg("-on");

        // Virtualised ARM64 Windows crashes out on PROFILE tracing, so this hidden
        // hack argument lets things still continue to run for development of samply.
        if !self.virtualized_aarch64_hack {
            xperf.arg("PROC_THREAD+LOADER+PROFILE+CSWITCH");
            xperf.arg("-stackwalk");
            xperf.arg("PROFILE+CSWITCH");
        } else {
            // virtualized arm64 hack, to give us enough interesting events
            xperf.arg("PROC_THREAD+LOADER+CSWITCH+SYSCALL+VIRT_ALLOC+OB_HANDLE");
            xperf.arg("-stackwalk");
            xperf.arg("VirtualAlloc+VirtualFree+HandleCreate+HandleClose");
        }
        xperf.arg("-f");
        xperf.arg(&kernel_etl_file);

        if let Some(user_etl_file) = &user_etl_file {
            xperf.arg("-start");
            xperf.arg("SamplySession");

            if self.capture_coreclr {
                panic!("No CoreCLR support yet!");
                //super::coreclr::add_coreclr_xperf_args(&mut xperf);
            }

            xperf.arg("-f");
            xperf.arg(user_etl_file);
        }

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
