//! This module contains internal provider implementations for compiling [samply-markers](crate) on Windows systems.
//!
//! Windows is not yet supported at this time.

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;
use crate::provider::TimestampNowProvider;
use crate::provider::WriteMarkerProvider;

/// A [`TimestampNowProvider`] that always panics because Windows is not supported yet.
pub struct TimestampNowImpl;

impl TimestampNowProvider for TimestampNowImpl {
    fn now() -> SamplyTimestamp {
        unimplemented!("samply-markers: this crate is not yet available on Windows");
    }
}

/// A [`WriteMarkerProvider`] that always panics because Windows is not supported yet.
pub struct WriteMarkerImpl;

impl WriteMarkerProvider for WriteMarkerImpl {
    fn write_marker(_start: SamplyTimestamp, _end: SamplyTimestamp, _marker: &SamplyMarker) {
        unimplemented!("samply-markers: this crate is not yet available on Windows");
    }
}
