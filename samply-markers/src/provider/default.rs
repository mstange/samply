//! This module contains default internal implementations that no-op when [samply-markers](crate) is not enabled.

use crate::marker::SamplyTimestamp;
use crate::provider::TimestampNowProvider;

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
