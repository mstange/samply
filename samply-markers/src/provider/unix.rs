use std::{
    cell::RefCell,
    fmt::{Display, Write as _},
    fs::File,
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use nix::{
    sys::mman::{MapFlags, ProtFlags},
    time::{ClockId, clock_gettime},
};
use smallstr::SmallString;
use tempfile::{TempDir, tempdir};
use tracing::error;

#[derive(Copy, Clone, Debug)]
struct Timestamp(i64);

impl Timestamp {
    fn now() -> Self {
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();
        Self(now.tv_sec() as i64 * 1_000_000_000 + now.tv_nsec() as i64)
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn markers_dir() -> Option<&'static Path> {
    static MARKERS_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
        tempdir()
            .map(TempDir::keep)
            .inspect_err(|e| error!("Failed to create temporary directory: {e}"))
            .ok()
    });

    Some(LazyLock::force(&MARKERS_DIR).as_ref()?)
}

fn with_marker_file<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut File) -> R,
{
    fn create_marker_file() -> Option<File> {
        let file_name = markers_dir()?.join(format!(
            "marker-{}-{}.txt",
            nix::unistd::getpid(),
            nix::unistd::gettid()
        ));

        // Create the file.
        let file = File::options()
            .create(true)
            .write(true)
            .read({
                // We aren't going to read from the file, so we ordinarily
                // wouldn't need read permission, but we're about mmap it and
                // mmap always requires read permission.
                true
            })
            .open(&file_name)
            .inspect_err(|e| error!("Failed to create marker file {e}: {}", file_name.display()))
            .ok()?;

        // Create an mmap for the file.  This cannot be skipped because it
        // triggers samply to read and interpret the file.  We will not use it
        // to write to the file (because writing to a text file via mmap is
        // painful and we haven't yet proved that it is a performance problem),
        // so it is not necessary to map it with any particular protection or
        // flags, so we use PROT_READ because that offers the fewest ways to
        // screw up.
        unsafe {
            nix::sys::mman::mmap(
                None,
                NonZeroUsize::new(4096).unwrap(),
                ProtFlags::PROT_READ,
                MapFlags::MAP_SHARED,
                &file,
                0,
            )
        }
        .inspect_err(|e| error!("Failed to mmap marker file {e}: {}", file_name.display()))
        .ok()?;

        Some(file)
    }

    thread_local! {
            static MARKER_FILE: RefCell<Option<File>> = RefCell::new(create_marker_file());
    }

    MARKER_FILE.with_borrow_mut(|file| file.as_mut().map(f))
}

fn write_marker(start: Timestamp, end: Timestamp, name: &str) {
    let mut s = SmallString::<[u8; 64]>::new();
    writeln!(&mut s, "{start} {end} {name}").unwrap();
    let _ = with_marker_file(|f| f.write_all(s.as_bytes()));
}
