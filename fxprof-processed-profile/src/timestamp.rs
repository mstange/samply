use std::io::Write;

use crate::writer::Writer;

/// The type used for sample and marker timestamps.
///
/// Timestamps in the profile are stored in reference to the profile's [`ReferenceTimestamp`](crate::ReferenceTimestamp).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Timestamp {
    nanos: u64,
}

impl Timestamp {
    /// Create a timestamp from nanoseconds since the profile's
    /// [`ReferenceTimestamp`](crate::ReferenceTimestamp).
    pub fn from_nanos_since_reference(nanos: u64) -> Self {
        Self { nanos }
    }

    /// Create a timestamp from fractional milliseconds since the profile's
    /// [`ReferenceTimestamp`](crate::ReferenceTimestamp).
    ///
    /// This is the unit used in the profile JSON, so it's the most natural form
    /// for callers that have already done the conversion.
    pub fn from_millis_since_reference(millis: f64) -> Self {
        Self {
            nanos: (millis * 1_000_000.0) as u64,
        }
    }

    /// The stored value as fractional milliseconds (the JSON unit).
    pub(crate) fn as_millis_f64(self) -> f64 {
        (self.nanos as f64) / 1_000_000.0
    }

    pub(crate) fn write_json<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.fp(self.as_millis_f64())
    }

    pub(crate) fn write_optional<W: Write>(
        ts: Option<Timestamp>,
        w: &mut Writer<W>,
    ) -> std::io::Result<()> {
        match ts {
            Some(ts) => w.fp(ts.as_millis_f64()),
            None => w.null_value(),
        }
    }
}

fn compute_timestamp_deltas_millis(times: &[Timestamp]) -> Vec<f64> {
    let mut deltas = Vec::with_capacity(times.len());
    let mut prev_nanos = 0u64;
    for ts in times {
        let cur = ts.nanos;
        let delta = cur - prev_nanos;
        prev_nanos = cur;
        deltas.push((delta as f64) / 1_000_000.0);
    }
    deltas
}

fn compute_timestamp_deltas_millis_with_permutation(
    times: &[Timestamp],
    indexes: &[usize],
) -> Vec<f64> {
    let mut deltas = Vec::with_capacity(indexes.len());
    let mut prev_nanos = 0u64;
    for &i in indexes {
        let cur = times[i].nanos;
        let delta = cur - prev_nanos;
        prev_nanos = cur;
        deltas.push((delta as f64) / 1_000_000.0);
    }
    deltas
}

/// Write timestamps as a `Float64Array` slab of deltas (in milliseconds).
pub fn write_timestamps_as_deltas<W: Write>(
    w: &mut Writer<W>,
    times: &[Timestamp],
) -> std::io::Result<()> {
    w.f64_array_owned(compute_timestamp_deltas_millis(times))
}

/// Write timestamps as a `Float64Array` slab of deltas (in milliseconds),
/// permuted by `indexes`.
pub fn write_timestamps_as_deltas_with_permutation<W: Write>(
    w: &mut Writer<W>,
    times: &[Timestamp],
    indexes: &[usize],
) -> std::io::Result<()> {
    w.f64_array_owned(compute_timestamp_deltas_millis_with_permutation(
        times, indexes,
    ))
}

/// Write `column` as a `Float64Array` slab of fractional-millisecond
/// timestamps, using `0.0` for `None`. (The value for `None` is ignored by
/// the front-end when the marker phase marks that endpoint as meaningless.)
pub fn write_optional_timestamp_column_as_zero_default<W: Write>(
    w: &mut Writer<W>,
    column: &[Option<Timestamp>],
) -> std::io::Result<()> {
    let millis: Vec<f64> = column
        .iter()
        .map(|ts| ts.map_or(0.0, Timestamp::as_millis_f64))
        .collect();
    w.f64_array_owned(millis)
}
