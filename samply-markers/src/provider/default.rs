//! This module contains default internal implementations that no-op when [samply-markers](crate) is not enabled.

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;
use crate::provider::TimestampNowProvider;
use crate::provider::WriteMarkerProvider;

/// A [`TimestampNowProvider`] that always reports zero when the default provider is active.
pub struct TimestampNowImpl;

impl TimestampNowProvider for TimestampNowImpl {
    /// Returns a zero timestamp.
    ///
    /// This function is intentionally not `const` because the real provider-specific
    /// implementations are not `const`. This keeps the API consistent even when
    /// [samply-markers](crate) is disabled.
    fn now() -> SamplyTimestamp {
        SamplyTimestamp::from_monotonic_nanos(0)
    }
}

/// A [`WriteMarkerProvider`] that does nothing when the default provider is active.
pub struct WriteMarkerImpl;

impl WriteMarkerProvider for WriteMarkerImpl {
    /// Does nothing with the marker data.
    fn write_marker(_start: SamplyTimestamp, _end: SamplyTimestamp, _marker: &SamplyMarker) {
        // no-op when markers are not enabled.
    }
}
