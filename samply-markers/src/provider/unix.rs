//! This module contains internal provider implementations for compiling [samply-markers](crate) on Unix systems.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;
use crate::provider::TimestampNowProvider;
use crate::provider::WriteMarkerProvider;

use nix::sys::mman::MapFlags;
use nix::sys::mman::ProtFlags;
#[cfg(target_os = "linux")]
use nix::time::ClockId;
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
static NANOS_PER_TICK: OnceLock<mach2::mach_time::mach_timebase_info> = OnceLock::new();

/// A monotonic nanosecond timestamp implementation.
pub struct TimestampNowImpl;

impl TimestampNowProvider for TimestampNowImpl {
    /// Queries the monotonic clock and converts the result to monotonic nanoseconds.
    fn now() -> SamplyTimestamp {
        #[cfg(target_os = "linux")]
        {
            let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();

            // Monotonic nanoseconds should only ever be positive.
            #[allow(clippy::cast_sign_loss)]
            SamplyTimestamp::from_monotonic_nanos(
                now.tv_sec() as u64 * 1_000_000_000 + now.tv_nsec() as u64,
            )
        }

        // Use mach_absolute_time() to match samply's clock source on macOS.
        // See https://github.com/mstange/samply/blob/2041b956f650bb92d912990052967d03fef66b75/samply/src/mac/time.rs#L8-L20
        #[cfg(target_os = "macos")]
        {
            use mach2::mach_time;

            let nanos_per_tick = NANOS_PER_TICK.get_or_init(|| {
                let mut info = mach_time::mach_timebase_info::default();
                // SAFETY: mach_timebase_info is an FFI call on macOS. We pass a valid mutable reference
                //         to a properly initialized mach_timebase_info struct.
                // See https://developer.apple.com/documentation/driverkit/3433733-mach_timebase_info
                let errno = unsafe { mach_time::mach_timebase_info(&raw mut info) };
                if errno != 0 || info.denom == 0 {
                    info.numer = 1;
                    info.denom = 1;
                }
                info
            });

            // SAFETY: mach_absolute_time is an FFI call on macOS that returns the current
            //         absolute time value in tick units.
            // See https://developer.apple.com/documentation/kernel/1462446-mach_absolute_time
            let time = unsafe { mach_time::mach_absolute_time() };
            let nanos = time * u64::from(nanos_per_tick.numer) / u64::from(nanos_per_tick.denom);

            SamplyTimestamp::from_monotonic_nanos(nanos)
        }
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

/// Returns a unique thread identifier for the current thread.
#[cfg(target_os = "linux")]
fn get_thread_id() -> u32 {
    // Thread IDs on Linux are always positive, so the cast from i32 to u32 is safe.
    #[allow(clippy::cast_sign_loss)]
    let tid = nix::unistd::gettid().as_raw() as u32;

    tid
}

/// Returns a unique thread identifier for the current thread.
#[cfg(target_os = "macos")]
fn get_thread_id() -> u32 {
    // Use pthread_threadid_np() to get the current thread's system thread ID.
    //
    // This is the simplest way to get our own thread ID. Samply uses thread_info()
    // instead because it's extracting thread IDs from other processes via mach ports.
    //
    // Both approaches return the same underlying system thread ID value.
    //
    // See https://github.com/mstange/samply/blob/2041b956f650bb92d912990052967d03fef66b75/samply/src/mac/thread_profiler.rs#L209-L229
    let mut tid: u64 = 0;

    // SAFETY: pthread_threadid_np is an FFI call. We pass pthread_self() provided by libc,
    //         along with a valid mutable reference to our tid variable.
    // See https://docs.rs/libc/latest/x86_64-apple-darwin/libc/fn.pthread_threadid_np.html
    unsafe {
        libc::pthread_threadid_np(libc::pthread_self(), &raw mut tid);
    }

    // Truncate to u32 to match samply's behavior.
    #[allow(clippy::cast_possible_truncation)]
    let tid = tid as u32;

    tid
}

/// Creates a new mmapped marker file for the current thread.
fn create_marker_file() -> Option<File> {
    let file_name = markers_dir()?.join(format!(
        "marker-{}-{}.txt",
        nix::unistd::getpid().as_raw(),
        get_thread_id()
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
    //
    // SAFETY: This call to mmap is safe because:
    //   - We're mapping a valid file descriptor that we just opened
    //   - The size (4096) is a valid, non-zero size
    //   - The offset is 0 which is valid for any file
    // See https://docs.rs/nix/latest/nix/sys/mman/fn.mmap.html
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
