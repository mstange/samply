//! This module contains integration tests for [`samply-markers`](crate) on Unix systems.

#![cfg(all(feature = "enabled", target_family = "unix"))]

use std::env;
use std::fs;
use std::thread;

use regex::Regex;
use samply_markers::prelude::*;
use serial_test::serial;

/// Returns a unique thread identifier for the current thread.
#[cfg(target_os = "linux")]
fn get_thread_id() -> u32 {
    nix::unistd::gettid().as_raw() as u32
}

/// Returns a unique thread identifier for the current thread.
#[cfg(target_os = "macos")]
fn get_thread_id() -> u32 {
    // Use pthread_threadid_np() to get the thread ID, same as samply does.
    // This returns a u64 but we cast to u32 to match samply's behavior and
    // ensure the thread ID fits in the marker filename format.
    unsafe {
        let mut tid: u64 = 0;
        libc::pthread_threadid_np(libc::pthread_self(), &mut tid);
        tid as u32
    }
}

/// Wraps a test body with automatic cleanup of marker files for the current process.
macro_rules! with_marker_file_cleanup {
    ($body:block) => {{
        let pid = nix::unistd::getpid().as_raw() as u32;
        cleanup_marker_files(pid);

        let result = $body;

        cleanup_marker_files(pid);
        result
    }};
}

/// Asserts that exactly one marker file exists for the given PID, and verifies that it belongs to the given TID.
/// Returns the marker file paths for further inspection.
macro_rules! assert_single_marker_file {
    ($pid:expr, $tid:expr) => {{
        let marker_files = find_all_marker_files($pid);
        assert_eq!(
            marker_files.len(),
            1,
            "Expected exactly one marker file for single-threaded test, found {}",
            marker_files.len()
        );

        let marker_file = &marker_files[0];
        let expected_pattern = format!("marker-{}-{}", $pid, $tid);
        assert!(
            marker_file
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&expected_pattern),
            "Expected marker file to be for TID {}, but found {:?}",
            $tid,
            marker_file
        );

        marker_files
    }};
}

/// Asserts that the marker file for the given PID/TID matches the expected regex pattern(s).
///
/// When patterns are provided in the `regex: { ... }` block, the macro will by default
/// verify that the file contains exactly that many lines (one per pattern).
/// This can be disabled by passing `exact_count: false`.
macro_rules! assert_marker_file {
    // Regex patterns with exact line count check (default behavior).
    ($pid:expr, $tid:expr, regex: { $($pattern:expr),+ $(,)? }) => {{
        assert_marker_file!($pid, $tid, regex: { $($pattern),+ }, exact_count: true)
    }};

    // Regex patterns with optional exact line count check.
    ($pid:expr, $tid:expr, regex: { $($pattern:expr),+ $(,)? }, exact_count: $exact:expr) => {{
        let patterns = &[$($pattern),+];
        let combined_pattern = format!("(?m){}", patterns.join("\n"));
        assert_marker_file!($pid, $tid, regex: &combined_pattern, pattern_count: patterns.len(), exact_count: $exact)
    }};

    // Internal implementation with pattern count and exact count flag.
    ($pid:expr, $tid:expr, regex: $regex:expr, pattern_count: $count:expr, exact_count: $exact:expr) => {{
        let marker_files = find_all_marker_files($pid);
        let pattern_prefix = format!("marker-{}-{}", $pid, $tid);

        let marker_file = marker_files
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&pattern_prefix)
            })
            .expect(&format!(
                "Expected to find marker file for PID {} TID {}.",
                $pid, $tid
            ));

        let contents = fs::read_to_string(&marker_file).expect("failed to read marker file");
        let regex = Regex::new($regex).expect("invalid regex pattern");

        assert!(
            regex.is_match(&contents),
            "Marker file contents did not match expected pattern:\n\
             Expected pattern:\n{}\n\
             Actual contents:\n{}",
            $regex,
            contents
        );

        if $exact && $count > 0 {
            let line_count = contents.lines().count();
            assert_eq!(
                line_count,
                $count,
                "Expected exactly {} lines in marker file, but found {}",
                $count,
                line_count
            );
        }

        contents
    }};
}

/// Helper function to find all marker files for a given PID in the system temp directory.
fn find_all_marker_files(pid: u32) -> Vec<std::path::PathBuf> {
    let temp_dir = env::temp_dir();
    let pattern_prefix = format!("marker-{pid}-");

    fs::read_dir(&temp_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .flat_map(|entry| {
            fs::read_dir(entry.path())
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|sub_entry| {
                    sub_entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&pattern_prefix)
                })
                .map(|sub_entry| sub_entry.path())
        })
        .collect()
}

/// Helper function to clean up all marker files for a given PID.
fn cleanup_marker_files(pid: u32) {
    let marker_files = find_all_marker_files(pid);
    for file in marker_files {
        let _ = fs::remove_file(file);
    }
}

#[test]
#[serial]
fn instant_writes_marker_to_file() {
    with_marker_file_cleanup!({
        samply_marker!({ name: "InstantMarker1" });
        samply_marker!({ name: "InstantMarker2" });
        samply_marker!({ name: "InstantMarker3" });

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        let contents = assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ InstantMarker1$",
                r"^\d+ \d+ InstantMarker2$",
                r"^\d+ \d+ InstantMarker3$",
            }
        );

        assert!(
            contents.lines().all(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let start: u64 = parts[0].parse().expect("start time should be a valid u64");
                let end: u64 = parts[1].parse().expect("end time should be a valid u64");
                start == end
            }),
            "Expected all instant markers to have same start and end time."
        );
    });
}

#[test]
#[serial]
#[should_panic(
    expected = "assertion `left == right` failed: Expected exactly 1 lines in marker file, but found 2"
)]
fn assert_marker_file_panics_on_wrong_line_count() {
    with_marker_file_cleanup!({
        // Emit exactly 2 markers.
        SamplyMarker::new("test marker").emit_instant();
        SamplyMarker::new("test marker").emit_instant();

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        // Try to assert only 1 pattern when there are 2 lines.
        assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ test marker$",
            }
        );
    });
}

#[test]
#[serial]
fn empty_name_defaults_to_unnamed_marker() {
    with_marker_file_cleanup!({
        SamplyMarker::new("").emit_instant();
        SamplyMarker::new(format!("")).emit_instant();

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ unnamed marker$",
                r"^\d+ \d+ unnamed marker$",
            }
        );
    });
}

#[test]
#[serial]
fn timer_writes_marker_to_file() {
    with_marker_file_cleanup!({
        {
            let _timer1 = samply_timer!({ name: "TimerMarker1" });
            thread::sleep(std::time::Duration::from_millis(2));
        }
        {
            let _timer2 = samply_timer!({ name: "TimerMarker2" });
            thread::sleep(std::time::Duration::from_millis(3));
        }
        {
            let _timer3 = samply_timer!({ name: "TimerMarker3" });
            thread::sleep(std::time::Duration::from_millis(4));
        }

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        let contents = assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ TimerMarker1$",
                r"^\d+ \d+ TimerMarker2$",
                r"^\d+ \d+ TimerMarker3$",
            }
        );

        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let start: u64 = parts[0].parse().expect("start time should be a valid u64");
            let end: u64 = parts[1].parse().expect("end time should be a valid u64");
            assert!(
                end >= start,
                "Expected the end time to be greater than or equal to the start time.",
            );
        }
    });
}

#[test]
#[serial]
fn measure_writes_marker_to_file() {
    with_marker_file_cleanup!({
        let result1 = samply_measure!({
            thread::sleep(std::time::Duration::from_millis(2));
            42
        } marker: {
            name: "MeasureMarker1",
        });

        let result2 = samply_measure!({
            thread::sleep(std::time::Duration::from_millis(3));
            "hello"
        } marker: {
            name: "MeasureMarker2",
        });

        let result3 = samply_measure!({
            thread::sleep(std::time::Duration::from_millis(4));
            vec![1, 2, 3]
        } marker: {
            name: "MeasureMarker3",
        });

        assert_eq!(result1, 42, "Expected measure to preserve return value.");
        assert_eq!(
            result2, "hello",
            "Expected measure to preserve return value."
        );
        assert_eq!(
            result3,
            vec![1, 2, 3],
            "Expected measure to preserve return value."
        );

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        let contents = assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ MeasureMarker1$",
                r"^\d+ \d+ MeasureMarker2$",
                r"^\d+ \d+ MeasureMarker3$",
            }
        );

        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let start: u64 = parts[0].parse().expect("start time should be a valid u64");
            let end: u64 = parts[1].parse().expect("end time should be a valid u64");
            assert!(
                end >= start,
                "Expected the end time {end} to be greater than or equal to the start time {start}.",
            );
        }
    });
}

#[test]
#[serial]
fn multiple_threads_create_separate_files() {
    with_marker_file_cleanup!({
        let handles = (0..3)
            .map(|i| {
                thread::spawn(move || {
                    samply_marker!({ name: format!("thread {i} marker A") });
                    samply_marker!({ name: format!("thread {i} marker B") });
                    samply_marker!({ name: format!("thread {i} marker C") });
                    thread::sleep(std::time::Duration::from_millis(10));

                    get_thread_id()
                })
            })
            .collect::<Vec<_>>();

        let tids: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect::<Vec<_>>();

        let pid = nix::unistd::getpid().as_raw() as u32;

        // Multi-threaded test should produce exactly one marker file per thread.
        let marker_files = find_all_marker_files(pid);
        assert_eq!(
            marker_files.len(),
            3,
            "Expected exactly 3 marker files (one per thread), found {}",
            marker_files.len()
        );

        for tid in tids {
            assert_marker_file!(
                pid,
                tid,
                regex: {
                    r"^\d+ \d+ thread \d marker A$",
                    r"^\d+ \d+ thread \d marker B$",
                    r"^\d+ \d+ thread \d marker C$",
                }
            );
        }
    });
}

#[tokio::test]
#[serial]
async fn measure_async_writes_marker_to_file() {
    use tokio::time::{Duration, sleep};

    with_marker_file_cleanup!({
        let result = samply_measure!(async {
            sleep(Duration::from_millis(5)).await;
            42
        }, marker: {
            name: "AsyncMeasure",
        })
        .await;

        assert_eq!(result, 42, "Expected measure to preserve return value.");

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        let contents = assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ AsyncMeasure$",
            }
        );

        let parts: Vec<&str> = contents.trim().split_whitespace().collect();
        let start: u64 = parts[0].parse().expect("start time should be a valid u64");
        let end: u64 = parts[1].parse().expect("end time should be a valid u64");
        assert!(
            end >= start,
            "Expected the end time {end} to be greater than or equal to the start time {start}.",
        );
    });
}

#[test]
#[serial]
fn timer_emit_prevents_double_emit_on_drop() {
    with_marker_file_cleanup!({
        {
            let timer = samply_timer!({ name: "ExplicitEmit" });
            thread::sleep(std::time::Duration::from_millis(2));
            timer.emit(); // Explicitly emit the timer.
            // The timer drops here, but should not emit a second time.
        }

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ ExplicitEmit$",
            }
        );
    });
}

#[tokio::test]
#[serial]
async fn measure_async_with_early_return_writes_marker() {
    use tokio::time::{Duration, sleep};

    with_marker_file_cleanup!({
        async fn fallible_operation(should_fail: bool) -> Result<String, &'static str> {
            samply_measure!(async {
                sleep(Duration::from_millis(2)).await;

                if should_fail {
                    return Err("operation failed");
                }

                sleep(Duration::from_millis(2)).await;
                Ok(String::from("success"))
            }, marker: {
                name: "FallibleAsync",
            })
            .await
        }

        let success_result = fallible_operation(false).await;
        assert!(success_result.is_ok());

        let failure_result = fallible_operation(true).await;
        assert!(failure_result.is_err());

        let pid = nix::unistd::getpid().as_raw() as u32;
        let tid = get_thread_id();

        assert_single_marker_file!(pid, tid);

        let contents = assert_marker_file!(
            pid,
            tid,
            regex: {
                r"^\d+ \d+ FallibleAsync$",
                r"^\d+ \d+ FallibleAsync$",
            }
        );

        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let start: u64 = parts[0].parse().expect("start time should be a valid u64");
            let end: u64 = parts[1].parse().expect("end time should be a valid u64");
            assert!(
                end >= start,
                "Expected the end time {end} to be greater than or equal to the start time {start}.",
            );
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[serial]
async fn tokio_spawn_writes_markers_across_threads() {
    use tokio::time::{Duration, sleep};

    with_marker_file_cleanup!({
        let handles = (0..3)
            .map(|i| {
                tokio::spawn(async move {
                    // Each spawned task may run on different threads.
                    // Emit multiple markers with awaits in between to allow potential thread migration.
                    samply_measure!(async {
                        sleep(Duration::from_millis(2)).await;
                    }, marker: {
                        name: format!("async task {i} marker A"),
                    })
                    .await;

                    samply_measure!(async {
                        sleep(Duration::from_millis(3)).await;
                    }, marker: {
                        name: format!("async task {i} marker B"),
                    })
                    .await;

                    samply_measure!(async {
                        sleep(Duration::from_millis(4)).await;
                    }, marker: {
                        name: format!("async task {i} marker C"),
                    })
                    .await;

                    // Return the thread ID where this task's final marker was emitted.
                    get_thread_id()
                })
            })
            .collect::<Vec<_>>();

        let _tids: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|result| result.expect("task panicked"))
            .collect();

        let pid = nix::unistd::getpid().as_raw() as u32;

        // Verify that markers were written. We can't predict which thread each task ran on
        // since tokio may schedule tasks on any worker thread, and tasks may migrate between
        // threads during execution. We collect all marker files for this PID to ensure we
        // find all markers regardless of which thread's file they were written to.
        let marker_files = find_all_marker_files(pid);

        // With 3 worker threads, we should have at least 1 file and at most 3 files.
        assert!(
            !marker_files.is_empty(),
            "Expected at least one marker file to exist"
        );
        assert!(
            marker_files.len() <= 3,
            "Expected at most 3 marker files are found {}",
            marker_files.len()
        );

        let all_contents = marker_files
            .into_iter()
            .filter_map(|path| fs::read_to_string(path).ok())
            .collect::<Vec<_>>()
            .join("\n");

        // Verify that all 9 markers (3 tasks × 3 markers each) appear somewhere
        for n in 0..3 {
            for marker in ["marker A", "marker B", "marker C"] {
                let expected = format!("async task {n} {marker}");
                assert!(
                    all_contents.contains(&expected),
                    "Expected to find '{expected}' in marker files but didn't.\nAll contents:\n{all_contents}"
                );
            }
        }
    });
}
