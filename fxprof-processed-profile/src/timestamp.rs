use serde::ser::{Serialize, Serializer};

/// The type used for sample and marker timestamps.
///
/// Timestamps in the profile are stored in reference to the profile's [`ReferenceTimestamp`](crate::ReferenceTimestamp).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Timestamp {
    nanos: u64,
}

impl Timestamp {
    pub fn from_nanos_since_reference(nanos: u64) -> Self {
        Self { nanos }
    }

    pub fn from_millis_since_reference(millis: f64) -> Self {
        Self {
            nanos: (millis * 1_000_000.0) as u64,
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // In the profile JSON, timestamps are currently expressed as float milliseconds
        // since profile.meta.startTime.
        serializer.serialize_f64((self.nanos as f64) / 1_000_000.0)
    }
}

pub struct SerializableTimestampSliceAsDeltas<'a>(pub &'a [Timestamp]);

impl Serialize for SerializableTimestampSliceAsDeltas<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut prev_nanos = 0;
        let delta_millis_iter = self.0.iter().map(|ts| {
            let cur_nanos = ts.nanos;
            let delta_nanos = cur_nanos - prev_nanos;
            prev_nanos = cur_nanos;
            (delta_nanos as f64) / 1_000_000.0
        });
        serializer.collect_seq(delta_millis_iter)
    }
}

pub struct SerializableTimestampSliceAsDeltasWithPermutation<'a>(
    pub &'a [Timestamp],
    pub &'a [usize],
);

impl Serialize for SerializableTimestampSliceAsDeltasWithPermutation<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut prev_nanos = 0;
        let delta_millis_iter = self.1.iter().map(|i| {
            let cur_nanos = self.0[*i].nanos;
            let delta_nanos = cur_nanos - prev_nanos;
            prev_nanos = cur_nanos;
            (delta_nanos as f64) / 1_000_000.0
        });
        serializer.collect_seq(delta_millis_iter)
    }
}
