use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;

use crate::serialization_helpers::{
    SerializableOptionalTimestampColumn, SerializableSingleValueColumn,
};
use crate::thread_string_table::ThreadInternalStringIndex;
use crate::{MarkerTiming, Timestamp};

#[derive(Debug, Clone, Default)]
pub struct MarkerTable {
    marker_name_string_indexes: Vec<ThreadInternalStringIndex>,
    marker_starts: Vec<Option<Timestamp>>,
    marker_ends: Vec<Option<Timestamp>>,
    marker_phases: Vec<Phase>,
    marker_datas: Vec<Value>,
}

impl MarkerTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn add_marker(
        &mut self,
        name: ThreadInternalStringIndex,
        timing: MarkerTiming,
        data: Value,
    ) {
        let (s, e, phase) = match timing {
            MarkerTiming::Instant(s) => (Some(s), None, Phase::Instant),
            MarkerTiming::Interval(s, e) => (Some(s), Some(e), Phase::Interval),
            MarkerTiming::IntervalStart(s) => (Some(s), None, Phase::IntervalStart),
            MarkerTiming::IntervalEnd(e) => (None, Some(e), Phase::IntervalEnd),
        };
        self.marker_name_string_indexes.push(name);
        self.marker_starts.push(s);
        self.marker_ends.push(e);
        self.marker_phases.push(phase);
        self.marker_datas.push(data);
    }
}

impl Serialize for MarkerTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.marker_name_string_indexes.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("category", &SerializableSingleValueColumn(0, len))?;
        map.serialize_entry("data", &self.marker_datas)?;
        map.serialize_entry(
            "endTime",
            &SerializableOptionalTimestampColumn(&self.marker_ends),
        )?;
        map.serialize_entry("name", &self.marker_name_string_indexes)?;
        map.serialize_entry("phase", &self.marker_phases)?;
        map.serialize_entry(
            "startTime",
            &SerializableOptionalTimestampColumn(&self.marker_starts),
        )?;
        map.end()
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Phase {
    Instant = 0,
    Interval = 1,
    IntervalStart = 2,
    IntervalEnd = 3,
}

impl Serialize for Phase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}
