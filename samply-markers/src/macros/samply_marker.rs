//! This module defines the [`samply_marker!`](crate::samply_marker) macro for emitting markers directly.

/// Emits a marker at the current location in the code's execution.
///
/// * Providing only a `name` will emit an instant marker at the current time.
/// * Providing an optional `start_time` will emit an interval marker from `start_time` to now.
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
/// samply_marker!({
///     name: format!("received request {}", request.id()),
/// });
///
/// // Mark the start time for serializing the response.
/// let start = SamplyTimestamp::now();
///
/// let data = query_database(request.id());
/// let json = serialize_json(&data);
///
/// // Emit an interval marker.
/// samply_marker!({
///     name: "serialize response",
///     start_time: start,
/// });
/// ```
#[macro_export]
macro_rules! samply_marker {
    // Instant marker with only a name.
    {{ name: $name:expr $(,)? }} => {{
        $crate::marker::SamplyMarker::new($name).emit_instant();
    }};

    // Interval marker with a provided start time.
    {{ name: $name:expr, start_time: $start_time:expr $(,)? }} => {{
        $crate::marker::SamplyMarker::new($name).emit_interval($start_time);
    }};
}
