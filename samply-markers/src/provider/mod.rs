//! This module contains context-specific, uniform implementations for the internal APIs used by this crate.
//!
//! The default provider is designed to no-op when the code is not compiled for profiling.
//!
//! When [samply-markers](crate) is enabled, there are platform-specific provider implementations for Unix and Windows systems.
//!
//! * An internal provider must provide a type [`TimestampNowImpl`] that implements the [`TimestampNowProvider`] trait.
//! * An internal provider must provide a type [`WriteMarkerImpl`] that implements the [`WriteMarkerProvider`] trait.
//!
//! [`TimestampNowProvider`]: crate::provider::TimestampNowProvider
//! [`WriteMarkerProvider`]: crate::provider::WriteMarkerProvider

use crate::marker::SamplyMarker;
use crate::marker::SamplyTimestamp;

pub use internal_provider::TimestampNowImpl;
pub use internal_provider::WriteMarkerImpl;

/// A trait implemented by context-specific providers to supply monotonic nanosecond timestamp.
pub trait TimestampNowProvider {
    /// Returns a monotonic timestamp in nanoseconds.
    fn now() -> SamplyTimestamp;
}

/// A trait implemented by context-specific providers to write markers for ingestion by samply.
pub trait WriteMarkerProvider {
    /// Writes a marker span for profiling consumption.
    fn write_marker(start: SamplyTimestamp, end: SamplyTimestamp, marker: &SamplyMarker);
}

#[cfg(not(feature = "enabled"))]
pub mod r#default;
#[cfg(not(feature = "enabled"))]
pub use r#default as internal_provider;

#[cfg(all(feature = "enabled", target_family = "unix"))]
pub mod unix;
#[cfg(all(feature = "enabled", target_family = "unix"))]
pub use unix as internal_provider;
