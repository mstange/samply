//! Runtime support for `samply`.
//!
//! The [samply] profiler allows the process that it is profiling to indicate
//! important runtime spans so that they show up in the profile for the thread
//! or the process.  The way that the profiled process does this is somewhat
//! arcane, so this module provides support for this on supported systems.
//!
//! Use [SamplySpan] to emit a span.
//!
//! # Viewing in the Firefox Profiler
//!
//! Spans logged by this module show up in the Marker Chart and Marker Table
//! tabs for a given thread. They are linked to particular threads and the
//! profiler will only show them when those threads are selected.  Those threads
//! *should* be the ones that emit them, but when `samply` attaches to a process
//! that is already running, it cannot tell which thread emitted them, so it
//! links all of them to the main thread (the one with the process name at the
//! top of the profiler window).  For Feldera pipelines, this usually means that
//! all of the spans will be linked to the main thread.
//!
//! See [example] output, which should show spans for `input`, `step`, and
//! `update` in the track for `program_0198a9c5-3b...`.
//!
//! [example]: https://profiler.firefox.com/public/5r65r8dg7estdv9sgd154qf2ta7japgp0hm7r50/marker-chart/?globalTrackOrder=0&hiddenLocalTracksByPid=1305600-0whjowx3x7xawzhznwAb&range=14344m640~14344m184&symbolServer=http%3A%2F%2F127.0.0.1%3A3001%2F3d9s4vp2mr2q30dw5rr3n09f7xgpds0mh8cr5nw&thread=0zjwzn&v=11
//!
//! # Enabling
//!
//! Logging must be configured to enable emitting spans at runtime:
//!
//! - Debug-level logging must be enabled for this module.
//!
//! - Logging must also be enabled for the particular spans of interest.  These
//!   are probably also at debug level.
//!
//! For Feldera pipelines, the easiest way to enable logging these spans is
//! probably to enable debug-level logging globally by setting `"logging":
//! "debug"` for the pipeline.
//!
//! # Performance
//!
//! There is some performance cost for emitting a span:
//!
//! - Each thread that emits a span writes a temporary file and retains a file
//!   descriptor for it.
//!
//! - Emitting a span takes one system call to write a line to the temporary
//!   file.  (In theory, this can be avoided using `mmap` to write the file, but
//!   writing text files with `mmap` is a nasty business and so far we avoid
//!   it.)
//!
//! # Cleanup
//!
//! The process can't delete its temporary files when it exits, because `samply`
//! needs to read them afterward.  Pass `--unlink-aux-files` to samply to have
//! it delete them after reading.
//!
//! This implementation writes the temporary files in a temporary directory.
//! Nothing deletes the temporary directory.
//!
//! [samply]: https://github.com/mstange/samply?tab=readme-ov-file#samply
#![warn(missing_docs)]
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
    time::{clock_gettime, ClockId},
};
use smallstr::SmallString;
use tempfile::{tempdir, TempDir};
use tracing::{enabled, error, span::EnteredSpan, Level, Span};

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

/// A [Span] that can also be emitted to [samply].
///
/// See [module documentation] for more information.
///
/// [samply]: https://github.com/mstange/samply?tab=readme-ov-file#samply
/// [module documentation]: crate::samply
pub struct SamplySpan {
    span: Span,
}

impl SamplySpan {
    /// Constructs a new [SamplySpan] that wraps `span`.
    pub fn new(span: Span) -> Self {
        Self { span }
    }

    /// Returns the inner [Span].
    pub fn into_inner(self) -> Span {
        self.span
    }

    /// Enters this span, consuming it and returning a
    /// [guard](EnteredSamplySpan) that will exit the span when dropped.
    ///
    /// If the span is enabled for [tracing] purposes, **and** this module is
    /// enabled at debug level, then the span will be emitted for consumption by
    /// the [samply] profiler.  See [module documentation] for more information.
    ///
    /// [module documentation]: crate::samply
    pub fn entered(self) -> EnteredSamplySpan {
        EnteredSamplySpan::new(self)
    }

    /// Executes the given function in the context of the span.
    ///
    /// If this span is enabled, then this function enters the span, invokes `f`
    /// and then exits the span. If the span is disabled, `f` will still be
    /// invoked, but in the context of the currently-executing span (if there is
    /// one).
    ///
    /// Returns the result of evaluating `f`.
    ///
    /// If the span is enabled for [tracing] purposes, **and** this module is
    /// enabled at debug level, then the span will be emitted for consumption by
    /// the [samply] profiler.  See [module documentation] for more information.
    ///
    /// [module documentation]: crate::samply
    pub fn in_scope<F, T>(self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let _entered_span = self.entered();
        f()
    }
}

/// An owned guard representing a span which has been entered and is currently
/// executing.
///
/// When the guard is dropped, the span will be exited.
///
/// This is returned by [SamplySpan::entered].
pub struct EnteredSamplySpan {
    span: EnteredSpan,
    start: Option<Timestamp>,
}

impl EnteredSamplySpan {
    fn new(span: SamplySpan) -> Self {
        Self {
            start: if span.span.is_disabled() || !enabled!(Level::DEBUG) {
                None
            } else {
                Some(Timestamp::now())
            },
            span: span.span.entered(),
        }
    }

    /// Exits the span, returning the underlying [SamplySpan].
    #[allow(dead_code)]
    pub fn exit(mut self) -> SamplySpan {
        self.do_exit();
        SamplySpan {
            span: std::mem::replace(&mut self.span, Span::none().entered()).exit(),
        }
    }

    fn do_exit(&mut self) {
        if let Some(start) = self.start.take() {
            write_marker(
                start,
                Timestamp::now(),
                self.span.metadata().unwrap().name(),
            );
        }
    }
}

impl Drop for EnteredSamplySpan {
    fn drop(&mut self) {
        self.do_exit();
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
