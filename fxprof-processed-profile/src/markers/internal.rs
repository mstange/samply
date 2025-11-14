use serde::ser::{Serialize, SerializeMap, SerializeSeq};

use crate::{CategoryHandle, Profile};

use super::dynamic_schema::{
    DynamicSchemaMarkerField, DynamicSchemaMarkerGraph, DynamicSchemaMarkerSchema,
};
use super::field_kind_counts::MarkerFieldKindCounts;
use super::serialization::{SerializableSchemaDisplay, SerializableSchemaField};
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
    fn serialize_self<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.type_name)?;
        map.serialize_entry("display", &SerializableSchemaDisplay(self.locations))?;
        if let Some(label) = &self.chart_label {
            map.serialize_entry("chartLabel", label)?;
        }
        if let Some(label) = &self.tooltip_label {
            map.serialize_entry("tooltipLabel", label)?;
        }
        if let Some(label) = &self.table_label {
            map.serialize_entry("tableLabel", label)?;
        }
        if let Some(description) = &self.description {
            map.serialize_entry("description", description)?;
        }
        map.serialize_entry("fields", &SerializableSchemaFields(self))?;
        if !self.graphs.is_empty() {
            map.serialize_entry("graphs", &self.graphs)?;
        }
        map.end()
    }

    fn serialize_fields<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for field in &self.fields {
            seq.serialize_element(&SerializableSchemaField(field))?;
        }
        seq.end()
    }
}

impl Serialize for InternalMarkerSchema {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.serialize_self(serializer)
    }
}

struct SerializableSchemaFields<'a>(&'a InternalMarkerSchema);

impl Serialize for SerializableSchemaFields<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize_fields(serializer)
    }
}
