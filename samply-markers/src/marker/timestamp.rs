//! This module contains the implementation of an opaque monotonic nanosecond timestamp used by [samply-markers](crate).

use crate::provider::TimestampNowImpl;
use crate::provider::TimestampNowProvider;

/// A monotonic timestamp expressed in nanoseconds.
///
/// This type is intentionally opaque to avoid comparison inconsistencies between builds
/// that have [samply-markers](crate) enabled or disabled.
///
/// # Examples
///
/// ```rust
/// # use samply_markers::marker::SamplyTimestamp;
/// let start = SamplyTimestamp::now();
/// ```
#[derive(Copy, Clone, Debug)]
#[cfg_attr(not(feature = "enabled"), allow(unused))]
pub struct SamplyTimestamp(u64);

impl SamplyTimestamp {
    /// Returns the current monotonic time in nanoseconds.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use samply_markers::marker::SamplyTimestamp;
    /// let start = SamplyTimestamp::now();
    /// ```
    #[inline]
    #[must_use]
    pub fn now() -> Self {
        <TimestampNowImpl as TimestampNowProvider>::now()
    }

    /// Creates a timestamp from the given nanoseconds of monotonic time.
    pub(crate) const fn from_monotonic_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Formats a [`SamplyTimestamp`] without implementing [`std::fmt::Display`].
    ///
    /// Having a dedicated formatter keeps [`SamplyTimestamp`] opaque, ensuring that it
    /// cannot be stringified via [`ToString`] and compared.
    #[cfg_attr(not(feature = "enabled"), allow(unused))]
    pub(crate) fn fmt<W>(self, writer: &mut W) -> std::fmt::Result
    where
        W: std::fmt::Write + ?Sized,
    {
        std::fmt::Write::write_fmt(writer, format_args!("{}", self.0))
    }
}

/// The following traits are implemented internally for testing only.
/// The [`SamplyTimestamp`] struct should remain opaque to consumers of this crate.
#[cfg(test)]
mod compare {
    use super::*;

    impl Eq for SamplyTimestamp {}
    impl PartialEq for SamplyTimestamp {
        fn eq(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }

    impl PartialOrd for SamplyTimestamp {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for SamplyTimestamp {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.cmp(&other.0)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn fmt_writes_correctly() {
        let time = SamplyTimestamp::from_monotonic_nanos(9876543210);
        let mut buffer = String::new();

        let result = time.fmt(&mut buffer);
        assert!(result.is_ok(), "Expected the fmt operation to succeed.");
        assert_eq!(
            buffer, "9876543210",
            "Expected the buffer to contain the formatted timestamp."
        );
    }

    #[test]
    #[cfg(feature = "enabled")]
    fn now_is_monotonic() {
        const ITERATIONS: usize = 1000;
        let mut timestamps = Vec::with_capacity(ITERATIONS);

        // Collect many timestamps in rapid succession.
        for _ in 0..ITERATIONS {
            timestamps.push(SamplyTimestamp::now());
        }

        // Verify monotonicity: each timestamp should be >= the previous.
        assert!(
            timestamps
                .iter()
                .zip(timestamps.iter().skip(1))
                .all(|(lhs, rhs)| lhs <= rhs),
            "Timestamps must be monotonically non-decreasing"
        );

        // At least some timestamps should be strictly increasing.
        let strictly_increasing_count = timestamps
            .iter()
            .zip(timestamps.iter().skip(1))
            .filter(|(lhs, rhs)| lhs < rhs)
            .count();

        assert!(
            strictly_increasing_count > 0,
            "Expected at least some timestamps to be strictly increasing, but all {} were equal",
            ITERATIONS
        );
    }

    #[test]
    #[cfg(not(feature = "enabled"))]
    fn now_returns_zero_when_disabled() {
        // When markers are disabled, timestamps should always return zero
        for _ in 0..10 {
            let ts = SamplyTimestamp::now();
            let mut buffer = String::new();
            ts.fmt(&mut buffer).unwrap();
            assert_eq!(
                buffer, "0",
                "Expected disabled timestamps to always be zero"
            );
        }
    }
}
