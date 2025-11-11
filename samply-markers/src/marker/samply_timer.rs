use std::borrow::Cow;

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;
use crate::provider::WriteMarkerImpl;
use crate::provider::WriteMarkerProvider;

/// A scoped timer that emits an interval marker when the current scope ends.
///
/// * The interval marker's start time is the moment the timer is created.
/// * The interval marker's end time is the moment the timer is dropped.
///
/// The interval marker can be purposely emitted before the end of the scope by invoking the [`emit()`] function.
///
/// * If [`emit()`] is called explicitly, it will not be called again when the timer is dropped.
///
/// # Examples
///
/// ```rust
/// # use samply_markers::marker::SamplyTimer;
/// # fn update_physics() {}
/// # fn render_scene() {}
/// # fn update_cache() {}
/// fn render_frame() {
///     // Create a timer to represent the entire function.
///     let _timer = SamplyTimer::new("render frame");
///
///     // Create a timer only for the physics and scene portion.
///     let render_segment = SamplyTimer::new("render scene");
///
///     update_physics();
///     render_scene();
///
///     // Emit here explicitly, instead of at the end of the frame.
///     render_segment.emit();
///
///     // Create a timer only for the cache portion.
///     let _cache_segment = SamplyTimer::new("update cache");
///     update_cache();
///
///     // The _cache_segment marker will be emitted automatically at the end of the frame.
///     // The _timer marker will be emitted automatically at the end of the frame.
/// }
///
/// render_frame();
/// ```
///
/// [`emit()`]: SamplyTimer::emit
#[derive(Debug)]
pub struct SamplyTimer<'data> {
    /// The marker that will be emitted when the timer is dropped.
    marker: SamplyMarker<'data>,
    /// The timestamp captured from when the timer was created.
    start_time: SamplyTimestamp,
}

impl<'data> SamplyTimer<'data> {
    /// Starts a new timer called `name`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::marker::SamplyTimer;
    /// # fn compute_result() {}
    /// fn expensive_function() {
    ///     let _timer = SamplyTimer::new("expensive computation");
    ///     compute_result();
    ///     // _timer emits when it is dropped at the end of scope.
    /// }
    /// #
    /// # expensive_function();
    /// ```
    ///
    /// ```rust
    /// # use samply_markers::marker::SamplyTimer;
    /// # fn process_batch(batch_id: u32) {}
    /// fn process_batches() {
    ///     for batch_id in 0..10 {
    ///         let _timer = SamplyTimer::new(format!("process batch {batch_id}"));
    ///         process_batch(batch_id);
    ///         // 10 interval markers are emitted, one for each loop iteration.
    ///     }
    /// }
    /// #
    /// # process_batches();
    /// ```
    #[inline]
    #[must_use]
    pub fn new(name: impl Into<Cow<'data, str>> + AsRef<str>) -> Self {
        Self {
            marker: SamplyMarker::new(name),
            start_time: SamplyTimestamp::now(),
        }
    }

    /// Emits the interval marker immediately, instead of waiting for the end of scope.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::marker::SamplyTimer;
    /// # fn expensive_computation() {}
    /// # fn log_results() {}
    /// # fn cleanup_temp_files() {}
    /// fn process_data() {
    ///     let timer = SamplyTimer::new("core computation");
    ///     expensive_computation();
    ///
    ///     // Emit the interval marker explicitly here.
    ///     timer.emit();
    ///
    ///     // These operations are not included in the interval marker
    ///     log_results();
    ///     cleanup_temp_files();
    ///
    ///     // The interval marker is not emitted again at the end of scope.
    /// }
    /// #
    /// # process_data();
    /// ```
    #[inline]
    pub fn emit(self) {
        drop(self);
    }
}

impl Drop for SamplyTimer<'_> {
    /// Emits the interval marker when the timer is dropped.
    #[inline]
    fn drop(&mut self) {
        let end = SamplyTimestamp::now();
        <WriteMarkerImpl as WriteMarkerProvider>::write_marker(self.start_time, end, &self.marker);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn new_with_str() {
        let timer = SamplyTimer::new("timer from str");
        assert_eq!(timer.marker.name(), "timer from str");
    }

    #[test]
    fn new_with_string() {
        let name = String::from("timer from String");
        let timer = SamplyTimer::new(name);
        assert_eq!(timer.marker.name(), "timer from String");
    }

    #[test]
    fn new_with_format() {
        let five = 5;
        let high = "high";
        let timer = SamplyTimer::new(format!("{high} {five}!"));
        assert_eq!(timer.marker.name(), "high 5!");
    }

    #[test]
    fn new_captures_start_time() {
        let before = SamplyTimestamp::now();

        std::thread::sleep(std::time::Duration::from_micros(10));
        let timer = SamplyTimer::new("timing test");
        std::thread::sleep(std::time::Duration::from_micros(10));

        let after = SamplyTimestamp::now();

        #[cfg(feature = "enabled")]
        assert!(
            timer.start_time > before,
            "Expected the start time {:?} to be greater than the before time {before:?}.",
            timer.start_time
        );
        #[cfg(not(feature = "enabled"))]
        assert!(
            timer.start_time >= before,
            "Expected the start time {:?} to be greater than or equal to the before time {before:?}.",
            timer.start_time
        );

        #[cfg(feature = "enabled")]
        assert!(
            timer.start_time < after,
            "Expected the start time {:?} to be less than the after time {after:?}.",
            timer.start_time
        );
        #[cfg(not(feature = "enabled"))]
        assert!(
            timer.start_time <= after,
            "Expected the start time {:?} to be less than or equal to the after time {after:?}.",
            timer.start_time
        );
    }
}
