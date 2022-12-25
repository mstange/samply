use serde::ser::{Serialize, Serializer};
use std::time::Duration;

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
    pub const ZERO: Self = Self { micros: 0 };

    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            micros: nanos / 1000,
        }
    }

    pub fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    pub fn from_millis(millis: f64) -> Self {
        Self {
            micros: (millis * 1_000.0) as u64,
        }
    }

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
