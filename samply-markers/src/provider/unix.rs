//! This module contains internal provider implementations for compiling [samply-markers](crate) on Unix systems.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;
use crate::provider::TimestampNowProvider;
use crate::provider::WriteMarkerProvider;

use nix::sys::mman::MapFlags;
use nix::sys::mman::ProtFlags;
use nix::time::ClockId;
use nix::time::clock_gettime;

use smallstr::SmallString;
use tempfile::TempDir;
use tempfile::tempdir;

/// A lazily created directory that holds per-thread mmapped marker files.
static MARKERS_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    tempdir()
        .map(TempDir::keep)
        .inspect_err(|e| eprintln!("samply-markers: failed to create temporary directory: {e}"))
        .ok()
});

thread_local! {
    /// A thread-local file used to append marker lines that samply will ingest.
    static MARKER_FILE: RefCell<Option<File>> = RefCell::new(create_marker_file());
}

/// A monotonic nanosecond timestamp backed by [`ClockId::CLOCK_MONOTONIC`].
pub struct TimestampNowImpl;

impl TimestampNowProvider for TimestampNowImpl {
    /// Queries `clock_gettime` and converts the result to monotonic nanoseconds.
    fn now() -> SamplyTimestamp {
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();

        // Monotonic nanoseconds should only ever be positive.
        #[allow(clippy::cast_sign_loss)]
        SamplyTimestamp::from_monotonic_nanos(
            now.tv_sec() as u64 * 1_000_000_000 + now.tv_nsec() as u64,
        )
    }
}

/// A marker writer that appends newline-delimited ranges to the per-thread file.
pub struct WriteMarkerImpl;

impl WriteMarkerProvider for WriteMarkerImpl {
    /// Serializes the marker to the thread-local file, creating it on demand.
    fn write_marker(start: SamplyTimestamp, end: SamplyTimestamp, marker: &SamplyMarker) {
        let mut s = SmallString::<[u8; 64]>::new();
        start.fmt(&mut s).unwrap();
        s.push(' ');
        end.fmt(&mut s).unwrap();
        s.push(' ');
        s.push_str(marker.name());
        s.push('\n');
        let _ = with_marker_file(|f| f.write_all(s.as_bytes()));
    }
}

/// Returns the lazily-created temporary markers directory, if available.
fn markers_dir() -> Option<&'static Path> {
    Some(LazyLock::force(&MARKERS_DIR).as_ref()?)
}

/// Executes the provided closure with a mutable handle to the thread-local marker file.
///
/// Returns [`None`] when the file could not be prepared.
fn with_marker_file<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut File) -> R,
{
    MARKER_FILE.with_borrow_mut(|file| file.as_mut().map(f))
}

/// Creates a new mmapped marker file for the current thread.
fn create_marker_file() -> Option<File> {
    let file_name = markers_dir()?.join(format!(
        "marker-{}-{}.txt",
        nix::unistd::getpid(),
        nix::unistd::gettid()
    ));

    let file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .read({
            // We aren't going to read from the file, so we ordinarily
            // wouldn't need read permission, but we're about mmap it and
            // mmap always requires read permission.
            true
        })
        .open(&file_name)
        .inspect_err(|e| {
            eprintln!(
                "samply-markers: failed to create marker file {e}: {}",
                file_name.display()
            );
        })
        .ok()?;

    // Create an mmap for the file. This cannot be skipped because it
    // triggers samply to read and interpret the file.  We will not use it
    // to write to the file (because writing to a text file via mmap is
    // painful and we haven't yet proved that it is a performance problem),
    // so it is not necessary to map it with any particular protection or
    // flags, so we use PROT_READ because that offers the fewest ways to
    // screw up.
    unsafe {
        nix::sys::mman::mmap(
            None,
            NonZeroUsize::new(4096).unwrap_unchecked(),
            ProtFlags::PROT_READ,
            MapFlags::MAP_SHARED,
            &file,
            0,
        )
    }
    .inspect_err(|e| {
        eprintln!(
            "samply-markers: failed to mmap marker file {e}: {}",
            file_name.display()
        );
    })
    .ok()?;

    Some(file)
}
