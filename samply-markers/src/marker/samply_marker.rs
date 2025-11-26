use std::borrow::Cow;
use std::cell::Cell;
use std::marker::PhantomData;

use crate::marker::SamplyTimestamp;
use crate::provider::WriteMarkerImpl;
use crate::provider::WriteMarkerProvider;

/// A marker that can emit at the current location in the code's execution.
///
/// * The marker may emit an instant, which represents a single point in time.
/// * The marker may emit an interval, which represents a span between a start time and end time.
///
/// # Examples
///
/// ```rust
/// # use samply_markers::prelude::*;
/// # struct Request {}
/// # impl Request { fn id(&self) -> u32 { 0 } }
/// # fn receive_request() -> Request { Request {} }
/// # fn query_database(id: u32) -> Vec<String> { vec![] }
/// # fn serialize_json(data: &[String]) -> String { String::new() }
/// let request = receive_request();
///
/// // Emit an instant marker.
/// SamplyMarker::new(format!("received request {}", request.id())).emit_instant();
///
/// // Mark the start time for serializing the response.
/// let start = SamplyTimestamp::now();
///
/// let data = query_database(request.id());
/// let json = serialize_json(&data);
///
/// // Emit an interval marker.
/// SamplyMarker::new("serialize response").emit_interval(start);
/// ```
#[derive(Debug)]
pub struct SamplyMarker<'data> {
    /// The name recorded with every marker emission.
    ///
    /// Defaults to `"unnamed marker"` if empty.
    name: Cow<'data, str>,

    /// This zero-sized member ensures that markers are `!Sync`.
    ///
    /// Markers are stil `Send`, allowing them to move between threads or be used in async tasks that migrate.
    /// When a marker is emitted, it records the TID of the thread that emits it, which accurately represents
    /// where the marker event occurred, even if the marker was created on a different thread.
    ///
    /// However, markers are `!Sync` because I see no valid use case for shared concurrent access to a marker.
    /// Markers are meant to be owned and emitted exactly once, not shared and emitted from multiple threads.
    ///
    /// If we need to remove this restriction in the future, it will be a non-breaking change, but adding this
    /// restriction would be a breaking change, so I would prefer to start more restrictively and increase the
    /// capabilities if there is an explicit need to do so.
    ///
    /// This can be removed in favor of `impl !Sync` if negative impls are ever stabilized.
    /// * <https://github.com/rust-lang/rust/issues/68318>
    _no_sync: PhantomData<Cell<()>>,
}

impl<'data> SamplyMarker<'data> {
    /// Creates a marker with the given `name`.
    ///
    /// Accepts [`&str`](str), [`String`], or anything convertible into [`Cow<'data, str>`].
    ///
    /// If `name` is an empty string, it will default to `"unnamed marker"`. While every marker _should_
    /// have a descriptive name, providing a default is preferable to panicking, since a third-party crate
    /// dependency could emit a marker with an empty name. Panicking in a scenario would mean that the code
    /// can no longer be profiled with markers enabled at all.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::prelude::*;
    /// SamplyMarker::new("database query").emit_instant();
    ///
    /// let user_id = 123;
    /// SamplyMarker::new(format!("fetch user {user_id}")).emit_instant();
    /// ```
    #[inline]
    #[must_use]
    pub fn new(name: impl Into<Cow<'data, str>> + AsRef<str>) -> Self {
        if name.as_ref().is_empty() {
            Self {
                name: Cow::Borrowed("unnamed marker"),
                _no_sync: PhantomData,
            }
        } else {
            Self {
                name: name.into(),
                _no_sync: PhantomData,
            }
        }
    }

    /// Returns the name of this marker.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::prelude::*;
    /// let marker = SamplyMarker::new("database query");
    /// assert_eq!(marker.name(), "database query");
    /// ```
    #[inline]
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Emits an instant marker at the current time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::prelude::*;
    /// # fn check_cache() -> Option<String> { None }
    /// # fn fetch_from_disk() -> String { String::new() }
    /// if let Some(data) = check_cache() {
    ///     SamplyMarker::new("cache hit").emit_instant();
    /// } else {
    ///     SamplyMarker::new("cache miss").emit_instant();
    ///     fetch_from_disk();
    /// }
    ///
    /// let request_id = 42;
    /// SamplyMarker::new(format!("request {request_id} completed")).emit_instant();
    /// ```
    #[inline]
    pub fn emit_instant(self) {
        let now = SamplyTimestamp::now();
        <WriteMarkerImpl as WriteMarkerProvider>::write_marker(now, now, &self);
    }

    /// Emits an interval marker from the given `start_time` to the current time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::prelude::*;
    /// # fn check_cache() -> Option<String> { None }
    /// # fn fetch_from_disk() -> String { String::new() }
    /// let start_time = SamplyTimestamp::now();
    ///
    /// let data = if let Some(cached_data) = check_cache() {
    ///     // Emit an interval only for the cache-check portion.
    ///     SamplyMarker::new("cache check").emit_interval(start_time);
    ///     cached_data
    /// } else {
    ///     // Emit an interval only for the cache-check portion.
    ///     SamplyMarker::new("cache check").emit_interval(start_time);
    ///
    ///     let fetch_start = SamplyTimestamp::now();
    ///     let fetched_data = fetch_from_disk();
    ///
    ///     // Emit an interval only for the fetch portion.
    ///     SamplyMarker::new("fetch from disk").emit_interval(fetch_start);
    ///
    ///     fetched_data
    /// };
    ///
    /// // Emit an interval for the total time, including cache check and fetch.
    /// SamplyMarker::new("total fetch time").emit_interval(start_time);
    /// ```
    #[inline]
    pub fn emit_interval(self, start_time: SamplyTimestamp) {
        let end = SamplyTimestamp::now();
        <WriteMarkerImpl as WriteMarkerProvider>::write_marker(start_time, end, &self);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn new_with_str() {
        let marker = SamplyMarker::new("test marker");
        assert_eq!(marker.name(), "test marker");
    }

    #[test]
    fn new_with_string() {
        let name = String::from("dynamic marker");
        let marker = SamplyMarker::new(name);
        assert_eq!(marker.name(), "dynamic marker");
    }

    #[test]
    fn new_with_format() {
        let five = 5;
        let high = "high";
        let marker = SamplyMarker::new(format!("{high} {five}!"));
        assert_eq!(marker.name(), "high 5!");
    }

    #[test]
    fn marker_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SamplyMarker>();
    }

    #[test]
    fn new_with_empty_name_defaults_to_unnamed_marker() {
        let marker = SamplyMarker::new("");
        assert_eq!(
            marker.name(),
            "unnamed marker",
            "Empty string should default to 'unnamed marker'"
        );
    }
}
