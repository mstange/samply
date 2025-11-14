use serde::ser::{Serialize, SerializeMap, SerializeSeq};

use super::dynamic_schema::DynamicSchemaMarkerField;
use super::types::MarkerLocations;

pub struct SerializableSchemaField<'a>(pub &'a DynamicSchemaMarkerField);

impl Serialize for SerializableSchemaField<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("key", &self.0.key)?;
        if !self.0.label.is_empty() {
            map.serialize_entry("label", &self.0.label)?;
        }
        map.serialize_entry("format", &self.0.format)?;
        map.end()
    }
}

pub struct SerializableSchemaDisplay(pub MarkerLocations);

impl Serialize for SerializableSchemaDisplay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        if self.0.contains(MarkerLocations::MARKER_CHART) {
            seq.serialize_element("marker-chart")?;
        }
        if self.0.contains(MarkerLocations::MARKER_TABLE) {
            seq.serialize_element("marker-table")?;
        }
        if self.0.contains(MarkerLocations::TIMELINE_OVERVIEW) {
            seq.serialize_element("timeline-overview")?;
        }
        if self.0.contains(MarkerLocations::TIMELINE_MEMORY) {
            seq.serialize_element("timeline-memory")?;
        }
        if self.0.contains(MarkerLocations::TIMELINE_IPC) {
            seq.serialize_element("timeline-ipc")?;
        }
        if self.0.contains(MarkerLocations::TIMELINE_FILEIO) {
            seq.serialize_element("timeline-fileio")?;
        }
        seq.end()
    }
}
