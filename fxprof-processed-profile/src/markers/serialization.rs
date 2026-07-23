use std::io::Write;

use super::dynamic_schema::DynamicSchemaMarkerField;
use super::types::MarkerLocations;
use crate::writer::Writer;

pub(super) fn write_schema_field<W: Write>(
    w: &mut Writer<W>,
    field: &DynamicSchemaMarkerField,
) -> std::io::Result<()> {
    w.object(|w| {
        w.name("key")?;
        w.string_value(&field.key)?;
        if !field.label.is_empty() {
            w.name("label")?;
            w.string_value(&field.label)?;
        }
        w.name("format")?;
        field.format.write_json(w)
    })
}

pub(super) fn write_schema_display<W: Write>(
    w: &mut Writer<W>,
    locations: MarkerLocations,
) -> std::io::Result<()> {
    w.array(|w| {
        if locations.contains(MarkerLocations::MARKER_CHART) {
            w.string_value("marker-chart")?;
        }
        if locations.contains(MarkerLocations::MARKER_TABLE) {
            w.string_value("marker-table")?;
        }
        if locations.contains(MarkerLocations::TIMELINE_OVERVIEW) {
            w.string_value("timeline-overview")?;
        }
        if locations.contains(MarkerLocations::TIMELINE_MEMORY) {
            w.string_value("timeline-memory")?;
        }
        if locations.contains(MarkerLocations::TIMELINE_IPC) {
            w.string_value("timeline-ipc")?;
        }
        if locations.contains(MarkerLocations::TIMELINE_FILEIO) {
            w.string_value("timeline-fileio")?;
        }
        Ok(())
    })
}
