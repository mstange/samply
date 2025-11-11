//! This module contains integration tests for [`samply-markers`](crate) on Windows systems.
//!
//! Windows support is not yet implemented, so these tests verify that attempting to use
//! markers on Windows results in a panic.

#![cfg(all(feature = "enabled", target_family = "windows"))]

use samply_markers::marker::SamplyMarker;
use samply_markers::marker::SamplyTimestamp;

#[test]
#[should_panic(expected = "not yet available on Windows")]
fn timestamp_now_panics() {
    let _ = SamplyTimestamp::now();
}

#[test]
#[should_panic(expected = "not yet available on Windows")]
fn emit_instant_panics() {
    SamplyMarker::new("test marker").emit_instant();
}
