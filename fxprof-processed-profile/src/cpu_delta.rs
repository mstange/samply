use serde::ser::{Serialize, Serializer};
use std::time::Duration;

/// The amount of CPU time between thread samples.
///
/// This is used in the Firefox Profiler UI to draw an activity graph per thread.
///
/// A thread only runs on one CPU at any time, and can get scheduled off and on
/// the CPU between two samples. The CPU delta is the accumulation of time it
/// was running on the CPU.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CpuDelta {
    micros: u64,
}

impl From<Duration> for CpuDelta {
    fn from(duration: Duration) -> Self {
        Self {
            micros: duration.as_micros() as u64,
        }
    }
}

impl CpuDelta {
    /// A CPU delta of zero.
    pub const ZERO: Self = Self { micros: 0 };

    /// Create a CPU delta from integer nanoseconds.
    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            micros: nanos / 1000,
        }
    }

    /// Create a CPU delta from integer microseconds.
    pub fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    /// Create a CPU delta from float milliseconds.
    pub fn from_millis(millis: f64) -> Self {
        Self {
            micros: (millis * 1_000.0) as u64,
        }
    }

    /// Whether the CPU delta is zero.
    pub fn is_zero(&self) -> bool {
        self.micros == 0
    }
}

impl Serialize for CpuDelta {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // CPU deltas are serialized as float microseconds, because
        // we set profile.meta.sampleUnits.threadCPUDelta to "µs".
        self.micros.serialize(serializer)
    }
}
