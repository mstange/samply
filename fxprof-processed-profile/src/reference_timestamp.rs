use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::ser::{Serialize, Serializer};

/// A timestamp which anchors the profile in absolute time.
///
/// In the profile JSON, this uses a UNIX timestamp.
///
/// All timestamps in the profile are relative to this reference timestamp.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq)]
pub struct ReferenceTimestamp {
    ms_since_unix_epoch: f64,
}

impl ReferenceTimestamp {
    /// Create a reference timestamp from a [`Duration`] since the UNIX epoch.
    pub fn from_duration_since_unix_epoch(duration: Duration) -> Self {
        Self::from_millis_since_unix_epoch(duration.as_secs_f64() * 1000.0)
    }

    /// Create a reference timestamp from milliseconds since the UNIX epoch.
    pub fn from_millis_since_unix_epoch(ms_since_unix_epoch: f64) -> Self {
        Self {
            ms_since_unix_epoch,
        }
    }

    /// Create a reference timestamp from a [`SystemTime`].
    pub fn from_system_time(system_time: SystemTime) -> Self {
        Self::from_duration_since_unix_epoch(system_time.duration_since(UNIX_EPOCH).unwrap())
    }
}

impl From<SystemTime> for ReferenceTimestamp {
    fn from(system_time: SystemTime) -> Self {
        Self::from_system_time(system_time)
    }
}

impl Serialize for ReferenceTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.ms_since_unix_epoch.serialize(serializer)
    }
}

/// An additional reference point in a platform-native clock.
///
/// This is supplementary to [`ReferenceTimestamp`] — the primary anchor for
/// the profile is still the wall-clock reference timestamp. Storing a
/// platform-specific reference value alongside it allows tools to correlate
/// sample or marker data from different data sources on the same timeline.
///
/// This enum is `#[non_exhaustive]`; new platforms may be added in minor releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlatformSpecificReferenceTimestamp {
    /// Linux / Android: a value from `clock_gettime(CLOCK_MONOTONIC)`, in
    /// nanoseconds.
    ClockMonotonicNanosecondsSinceBoot(u64),
    /// macOS / iOS: a value from `mach_absolute_time()`, converted to
    /// nanoseconds (via `mach_timebase_info`).
    MachAbsoluteTimeNanoseconds(u64),
    /// Windows: a raw value from `QueryPerformanceCounter`. The unit is
    /// performance-counter ticks (not nanoseconds); the tick rate is not stored.
    QueryPerformanceCounterValue(u64),
}
