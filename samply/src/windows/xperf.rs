use std::path::{Path, PathBuf};

pub struct Xperf {
    arch: String,
    xperf_path: PathBuf,
    state: XperfState,
}

enum XperfState {
    Stopped,
    RecordingToFile(PathBuf),
}

impl Xperf {
    pub fn new(arch: String) -> Result<Self, which::Error> {
        let xperf_path = which::which("xperf")?;
        Ok(Self {
            xperf_path,
            arch,
            state: XperfState::Stopped,
        })
    }

    pub fn is_running(&self) -> bool {
        matches!(&self.state, XperfState::RecordingToFile(_))
    }

    pub fn start_xperf(&mut self, output_file: &Path) {
        if self.is_running() {
            self.stop_xperf();
        }

        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let mut etl_file = output_file.to_path_buf();
        etl_file.set_extension("unmerged-etl");

        let mut xperf = runas::Command::new(&self.xperf_path);
        // Virtualised ARM64 Windows crashes out on PROFILE tracing, and that's what I'm developing
        // on, so these are hacky args to get me a useful profile that I can work with.
        xperf.arg("-on");
        if self.arch != "aarch64" {
            xperf.arg("PROC_THREAD+LOADER+PROFILE+CSWITCH");
        } else {
            xperf.arg("PROC_THREAD+LOADER+CSWITCH+SYSCALL+VIRT_ALLOC+OB_HANDLE");
        }
        xperf.arg("-stackwalk");
        if self.arch != "aarch64" {
            xperf.arg("PROFILE+CSWITCH");
        } else {
            xperf.arg("VirtualAlloc+VirtualFree+HandleCreate+HandleClose");
        }
        xperf.arg("-f");
        xperf.arg(expand_full_filename_with_cwd(&etl_file));

        let _ = xperf.status().expect("failed to execute xperf");

        eprintln!("xperf session running...");

        self.state = XperfState::RecordingToFile(PathBuf::from(&etl_file));
    }

    pub fn stop_xperf(&mut self) -> Option<PathBuf> {
        let prev_state = std::mem::replace(&mut self.state, XperfState::Stopped);
        let unmerged_etl = match prev_state {
            XperfState::Stopped => return None,
            XperfState::RecordingToFile(path) => path,
        };
        let merged_etl = unmerged_etl.with_extension("etl");

        let mut xperf = runas::Command::new(&self.xperf_path);
        xperf.arg("-stop");
        xperf.arg("-d");
        xperf.arg(expand_full_filename_with_cwd(&merged_etl));

        let _ = xperf
            .status()
            .expect("Failed to execute xperf -stop! xperf may still be recording.");

        eprintln!("xperf session stopped.");

        std::fs::remove_file(&unmerged_etl).unwrap_or_else(|_| {
            panic!(
                "Failed to delete unmerged ETL file {:?}",
                unmerged_etl.to_str().unwrap()
            )
        });

        Some(merged_etl)
    }
}

impl Drop for Xperf {
    fn drop(&mut self) {
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
