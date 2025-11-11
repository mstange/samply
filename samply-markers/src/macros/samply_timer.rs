//! This module defines the [`samply_timer!`](crate::samply_timer) macro for scope-based interval timing.

/// Creates a scoped timer that emits an interval marker when the current scope ends.
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
/// # use samply_markers::samply_timer;
/// # fn update_physics() {}
/// # fn render_scene() {}
/// # fn update_cache() {}
/// fn render_frame() {
///     // Create a timer to represent the entire function.
///     let _timer = samply_timer!({ name: "render frame" });
///
///     // Create a timer only for the physics and scene portion.
///     let render_segment = samply_timer!({ name: "render scene" });
///
///     update_physics();
///     render_scene();
///
///     // Emit here explicitly, instead of at the end of the frame.
///     render_segment.emit();
///
///     // Create a timer only for the cache portion.
///     let cache_segment = samply_timer!({ name: "update cache" });
///     update_cache();
///
///     // The cache_segment timer will be emitted automatically at the end of the frame.
///     // The _timer will be emitted automatically at the end of the frame.
/// }
///
/// render_frame();
/// ```
///
/// [`emit()`]: crate::marker::SamplyTimer::emit
///
#[macro_export]
macro_rules! samply_timer {
    {{ name: $name:expr $(,)? }} => {
        $crate::marker::SamplyTimer::new($name);
    };
}
