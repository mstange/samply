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

/// Write timestamps as a `Float64Array` slab of deltas (in milliseconds).
pub fn write_timestamps_as_deltas<'p, W: Write>(
    w: &mut Writer<'_, 'p, W>,
    times: &'p [Timestamp],
) -> std::io::Result<()> {
    let iter = times.iter().scan(0u64, |prev, ts| {
        let cur = ts.nanos;
        let delta = cur - *prev;
        *prev = cur;
        Some((delta as f64) / 1_000_000.0)
    });
    w.f64_array_from_iter(times.len(), iter)
}

/// Write timestamps as a `Float64Array` slab of deltas (in milliseconds),
/// permuted by `indexes`.
///
/// Takes `indexes` by value so the resulting scan iterator only borrows
/// `times` (which has the builder's lifetime `'p`) — the owned
/// `IntoIter<usize>` inside the iterator carries no lifetime constraint,
/// so nothing needs to be materialized into a `Vec<f64>` up front.
pub fn write_timestamps_as_deltas_with_permutation<'p, W: Write>(
    w: &mut Writer<'_, 'p, W>,
    times: &'p [Timestamp],
    indexes: Vec<usize>,
) -> std::io::Result<()> {
    let count = indexes.len();
    let iter = indexes.into_iter().scan(0u64, move |prev, i| {
        let cur = times[i].nanos;
        let delta = cur - *prev;
        *prev = cur;
        Some((delta as f64) / 1_000_000.0)
    });
    w.f64_array_from_iter(count, iter)
}

/// Write `column` as a `Float64Array` slab of fractional-millisecond
/// timestamps, using `0.0` for `None`. (The value for `None` is ignored by
/// the front-end when the marker phase marks that endpoint as meaningless.)
pub fn write_optional_timestamp_column_as_zero_default<'p, W: Write>(
    w: &mut Writer<'_, 'p, W>,
    column: &'p [Option<Timestamp>],
) -> std::io::Result<()> {
    let iter = column
        .iter()
        .map(|ts| ts.map_or(0.0, Timestamp::as_millis_f64));
    w.f64_array_from_iter(column.len(), iter)
}
