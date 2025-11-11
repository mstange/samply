//! This module contains context-specific, uniform implementations for the internal APIs used by this crate.
//!
//! The default provider is designed to no-op when the code is not compiled for profiling.
//!
//! When [samply-markers](crate) is enabled, there are platform-specific provider implementations for Unix and Windows systems.
//!
//! * An internal provider must provide a type [`TimestampNowImpl`] that implements the [`TimestampNowProvider`] trait.
//!
//! [`TimestampNowProvider`]: crate::provider::TimestampNowProvider

use crate::marker::SamplyTimestamp;

pub use internal_provider::TimestampNowImpl;

/// A trait implemented by context-specific providers to supply monotonic nanosecond timestamp.
pub trait TimestampNowProvider {
    /// Returns a monotonic timestamp in nanoseconds.
    fn now() -> SamplyTimestamp;
}

#[cfg(not(feature = "enabled"))]
pub mod r#default;
#[cfg(not(feature = "enabled"))]
pub use r#default as internal_provider;
