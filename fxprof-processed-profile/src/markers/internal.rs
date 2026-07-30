use std::io::Write;

use crate::writer::Writer;
use crate::{CategoryHandle, Profile};

use super::dynamic_schema::{
    DynamicSchemaMarkerField, DynamicSchemaMarkerGraph, DynamicSchemaMarkerSchema,
};
use super::field_kind_counts::MarkerFieldKindCounts;
use super::serialization::{write_schema_display, write_schema_field};
use super::static_schema::{Marker, MarkerFieldsTrait};
use super::types::MarkerLocations;

#[derive(Debug, Clone)]
pub struct InternalMarkerSchema {
    /// The name of this marker type.
    type_name: String,

    category: CategoryHandle,

    /// List of marker display locations.
    locations: MarkerLocations,

    chart_label: Option<String>,
    tooltip_label: Option<String>,
    table_label: Option<String>,

    /// The marker fields. These can be specified on each marker.
    fields: Vec<DynamicSchemaMarkerField>,

    /// Any graph tracks created from markers of this type
    graphs: Vec<DynamicSchemaMarkerGraph>,

    field_kind_counts: MarkerFieldKindCounts,

    description: Option<String>,
}

impl From<DynamicSchemaMarkerSchema> for InternalMarkerSchema {
    fn from(schema: DynamicSchemaMarkerSchema) -> Self {
        Self::from_runtime_schema(schema)
    }
}

impl InternalMarkerSchema {
    pub fn from_runtime_schema(schema: DynamicSchemaMarkerSchema) -> Self {
        let mut field_kind_counts = MarkerFieldKindCounts::new();
        for field in &schema.fields {
            field_kind_counts.add(field.format.kind());
        }
        Self {
            type_name: schema.type_name,
            category: schema.category,
            locations: schema.locations,
            chart_label: schema.chart_label,
            tooltip_label: schema.tooltip_label,
            table_label: schema.table_label,
            fields: schema.fields,
            graphs: schema.graphs,
            field_kind_counts,
            description: schema.description,
        }
    }

    pub fn from_static_schema<T: Marker>(profile: &mut Profile) -> Self {
        Self {
            type_name: T::UNIQUE_MARKER_TYPE_NAME.into(),
            category: profile.handle_for_category(T::CATEGORY),
            locations: T::LOCATIONS,
            chart_label: T::CHART_LABEL.map(Into::into),
            tooltip_label: T::TOOLTIP_LABEL.map(Into::into),
            table_label: T::TABLE_LABEL.map(Into::into),
            fields: <T::FieldsType as MarkerFieldsTrait>::to_runtime_field_schema(&T::FIELDS.0),
            field_kind_counts: T::FieldsType::FIELD_KIND_COUNTS,
            description: T::DESCRIPTION.map(Into::into),
            graphs: T::GRAPHS.iter().map(Into::into).collect(),
        }
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }
    pub fn category(&self) -> CategoryHandle {
        self.category
    }
    pub fn fields(&self) -> &[DynamicSchemaMarkerField] {
        &self.fields
    }
    pub fn string_field_count(&self) -> usize {
        self.field_kind_counts.string_field_count
    }
    pub fn number_field_count(&self) -> usize {
        self.field_kind_counts.number_field_count
    }
    pub fn flow_field_count(&self) -> usize {
        self.field_kind_counts.flow_field_count
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            w.name("name")?;
            w.string_value(&self.type_name)?;
            w.name("display")?;
            write_schema_display(w, self.locations)?;
            if let Some(label) = &self.chart_label {
                w.name("chartLabel")?;
                w.string_value(label)?;
            }
            if let Some(label) = &self.tooltip_label {
                w.name("tooltipLabel")?;
                w.string_value(label)?;
            }
            if let Some(label) = &self.table_label {
                w.name("tableLabel")?;
                w.string_value(label)?;
            }
            if let Some(description) = &self.description {
                w.name("description")?;
                w.string_value(description)?;
            }
            w.name("fields")?;
            w.array(|w| {
                for field in &self.fields {
                    write_schema_field(w, field)?;
                }
                Ok(())
            })?;
            if !self.graphs.is_empty() {
                w.name("graphs")?;
                w.array(|w| {
                    for graph in &self.graphs {
                        graph.write_json(w)?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
    }
}
