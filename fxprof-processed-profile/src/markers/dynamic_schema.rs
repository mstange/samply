use serde::ser::Serialize;
use serde_derive::Serialize;

use crate::{CategoryHandle, Profile, StringHandle};

use super::field_format::{
    MarkerFlowFieldFormat, MarkerNumberFieldFormat, MarkerStringFieldFormat,
};
use super::types::{
    GraphColor, MarkerFieldKind, MarkerGraphType, MarkerLocations, MarkerTypeHandle,
};

/// The trait for markers. You can implement [`Marker`](super::static_schema::Marker)
/// instead, which will give you an implementation of [`DynamicSchemaMarker`] via
/// a blanket impl.
///
/// Markers have a type, a name, a category, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`DynamicSchemaMarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// You can implement this trait manually if the schema of your marker type is only
/// known at runtime. If the schema is known at compile time, you'll want to implement
/// [`Marker`](super::static_schema::Marker) instead - there is a blanket impl which implements [`DynamicSchemaMarker`]
/// for any type that implements [`Marker`](super::static_schema::Marker).
pub trait DynamicSchemaMarker {
    /// The [`MarkerTypeHandle`] of this marker type. Implementations will usually call
    /// [`Profile::register_marker_type`] or [`Profile::static_schema_marker_type`].
    fn marker_type(&self, profile: &mut Profile) -> MarkerTypeHandle;

    /// The "name" of this marker, as an interned string handle.
    ///
    /// The name is shown as the row label in the marker chart. It can also be
    /// used as `{marker.name}` in the various `label` template strings in the schema.
    fn name(&self, profile: &mut Profile) -> StringHandle;

    /// Feed the values stored in this marker into the consumer,
    /// by calling its `consume_xyz_field` methods in the right order.
    ///
    /// The order has to match the order declared in the schema.
    fn push_field_values(&self, consumer: &mut impl MarkerFieldValueConsumer);
}

/// The field definition of a marker field, used in [`DynamicSchemaMarkerSchema::fields`].
///
/// For each marker which uses this schema, the value for this field is supplied by the
/// marker's implementation of [`push_field_values`](DynamicSchemaMarker::push_field_values).
///
/// Used with runtime-generated marker schemas. Use [`MarkerField`](super::static_schema::MarkerField) when using [`Marker`](super::static_schema::Marker).
#[derive(Debug, Clone)]
pub struct DynamicSchemaMarkerField {
    /// The field key. Must not be `type` or `cause`.
    pub key: String,

    /// The user-visible label of this field.
    pub label: String,

    /// The format of this field.
    pub format: DynamicSchemaMarkerFieldFormat,
}

/// Describes a marker type, including the names and types of the marker's fields.
/// You only need this if you don't know the schema until runtime. Otherwise, use
/// [`Marker`](super::static_schema::Marker) instead.
///
/// Example:
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocations, MarkerStringFieldFormat, MarkerNumberFieldFormat,
///     DynamicSchemaMarkerSchema, DynamicSchemaMarkerField,
///     CategoryHandle, StringHandle, Category, CategoryColor,
/// };
///
/// # fn fun() {
/// # let mut profile = Profile::new("My app", std::time::SystemTime::now().into(), fxprof_processed_profile::SamplingInterval::from_millis(1));
/// let schema = DynamicSchemaMarkerSchema {
///     type_name: "custom".into(),
///     category: profile.handle_for_category(Category("Custom", CategoryColor::Purple)),
///     locations: MarkerLocations::MARKER_CHART | MarkerLocations::MARKER_TABLE,
///     chart_label: Some("{marker.data.eventName}".into()),
///     tooltip_label: Some("Custom {marker.name} marker".into()),
///     table_label: Some("{marker.name} - {marker.data.eventName} with allocation size {marker.data.allocationSize} (latency: {marker.data.latency})".into()),
///     fields: vec![
///         DynamicSchemaMarkerField {
///             key: "eventName".into(),
///             label: "Event name".into(),
///             format: MarkerStringFieldFormat::String.into(),
///         },
///         DynamicSchemaMarkerField {
///             key: "allocationSize".into(),
///             label: "Allocation size".into(),
///             format: MarkerNumberFieldFormat::Bytes.into(),
///         },
///         DynamicSchemaMarkerField {
///             key: "url".into(),
///             label: "URL".into(),
///             format: MarkerStringFieldFormat::Url.into(),
///         },
///         DynamicSchemaMarkerField {
///             key: "latency".into(),
///             label: "Latency".into(),
///             format: MarkerNumberFieldFormat::Duration.into(),
///         },
///     ],
///     description: Some("This is a test marker with a custom schema.".into()),
///     graphs: vec![],
/// };
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct DynamicSchemaMarkerSchema {
    /// The unique name of this marker type. There must not be any other schema
    /// with the same name.
    pub type_name: String,

    /// The category shared by all markers of this schema.
    pub category: CategoryHandle,

    /// An optional description string. Applies to all markers of this type.
    pub description: Option<String>,

    /// Set of marker display locations.
    pub locations: MarkerLocations,

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// If set to `None`, the boxes in the marker chart will be empty.
    pub chart_label: Option<String>,

    /// A template string defining the label shown in the first row of the marker's tooltip.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`.
    pub tooltip_label: Option<String>,

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`.
    pub table_label: Option<String>,

    /// The marker fields. The values are supplied by each marker, in the marker's
    /// implementations of `push_field_values`.
    pub fields: Vec<DynamicSchemaMarkerField>,

    /// Any graph lines / segments created from markers of this type.
    ///
    /// If this is non-empty, the Firefox Profiler will create one graph track per
    /// marker *name*, per thread, based on the markers it finds on that thread.
    /// The marker name becomes the track's label.
    ///
    /// The elements in the graphs array describe individual graph lines or bar
    /// chart segments which are all drawn inside the same track, stacked on top of
    /// each other, in the order that they're listed here, with the first entry
    /// becoming the bottom-most graph within the track.
    pub graphs: Vec<DynamicSchemaMarkerGraph>,
}

/// A graph within a marker graph track, used in [`DynamicSchemaMarkerSchema::graphs`].
///
/// Used with runtime-generated marker schemas. Use [`MarkerGraph`](super::static_schema::MarkerGraph)
/// when using [`Marker`](super::static_schema::Marker).
#[derive(Clone, Debug, Serialize)]
pub struct DynamicSchemaMarkerGraph {
    /// The key of a number field that's declared in the marker schema.
    ///
    /// The values of this field are the values of this graph line /
    /// bar graph segment.
    pub key: String,
    /// Whether this marker graph segment is a line or a bar graph segment.
    #[serde(rename = "type")]
    pub graph_type: MarkerGraphType,
    /// The color of the graph segment. If `None`, the choice is up to the front-end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<GraphColor>,
}

/// Passed to [`DynamicSchemaMarker::push_field_values`].
pub trait MarkerFieldValueConsumer {
    fn consume_string_field(&mut self, string_handle: StringHandle);
    fn consume_number_field(&mut self, number: f64);
    fn consume_flow_field(&mut self, flow: u64);
}

// A combined enum for the format enums of all field kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicSchemaMarkerFieldFormat {
    String(MarkerStringFieldFormat),
    Number(MarkerNumberFieldFormat),
    Flow(MarkerFlowFieldFormat),
}

impl DynamicSchemaMarkerFieldFormat {
    pub fn kind(&self) -> MarkerFieldKind {
        match self {
            DynamicSchemaMarkerFieldFormat::String(_) => MarkerFieldKind::String,
            DynamicSchemaMarkerFieldFormat::Number(_) => MarkerFieldKind::Number,
            DynamicSchemaMarkerFieldFormat::Flow(_) => MarkerFieldKind::Flow,
        }
    }
}

impl Serialize for DynamicSchemaMarkerFieldFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            DynamicSchemaMarkerFieldFormat::String(marker_string_field_format) => {
                marker_string_field_format.serialize(serializer)
            }
            DynamicSchemaMarkerFieldFormat::Number(marker_number_field_format) => {
                marker_number_field_format.serialize(serializer)
            }
            DynamicSchemaMarkerFieldFormat::Flow(marker_flow_field_format) => {
                marker_flow_field_format.serialize(serializer)
            }
        }
    }
}

impl From<MarkerStringFieldFormat> for DynamicSchemaMarkerFieldFormat {
    fn from(format: MarkerStringFieldFormat) -> Self {
        Self::String(format)
    }
}

impl From<MarkerNumberFieldFormat> for DynamicSchemaMarkerFieldFormat {
    fn from(format: MarkerNumberFieldFormat) -> Self {
        Self::Number(format)
    }
}

impl From<MarkerFlowFieldFormat> for DynamicSchemaMarkerFieldFormat {
    fn from(format: MarkerFlowFieldFormat) -> Self {
        Self::Flow(format)
    }
}
