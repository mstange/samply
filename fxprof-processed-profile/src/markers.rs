/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this file,
 * You can obtain one at http://mozilla.org/MPL/2.0/. */

use bitflags::bitflags;
use serde::ser::{Serialize, SerializeMap, SerializeSeq};
use serde_derive::Serialize;

use super::profile::StringHandle;
use super::timestamp::Timestamp;
use crate::{CategoryHandle, Profile};

/// The handle for a marker. Returned from [`Profile::add_marker`].
///
/// This allows adding a stack to marker after the marker has been added.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct MarkerHandle(pub(crate) usize);

/// The handle for a marker type. Returned from [`Profile::register_marker_type`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct MarkerTypeHandle(pub(crate) usize);

/// Specifies timestamps for a marker.
#[derive(Debug, Clone)]
pub enum MarkerTiming {
    /// Instant markers describe a single point in time.
    Instant(Timestamp),
    /// Interval markers describe a time interval with a start and end timestamp.
    Interval(Timestamp, Timestamp),
    /// A marker for just the start of an actual marker. Can be paired with an
    /// `IntervalEnd` marker of the same name; if no end marker is supplied, this
    /// creates a marker that extends to the end of the profile.
    ///
    /// This can be used for long-running markers for pieces of activity that may
    /// not have completed by the time the profile is captured.
    IntervalStart(Timestamp),
    /// A marker for just the end of an actual marker. Can be paired with an
    /// `IntervalStart` marker of the same name; if no start marker is supplied,
    /// this creates a marker which started before the beginning of the profile.
    ///
    /// This can be used to mark pieces of activity which started before profiling
    /// began.
    IntervalEnd(Timestamp),
}

/// The marker trait. You'll likely want to implement [`StaticSchemaMarker`] instead.
///
/// Markers have a type, a name, a category, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`RuntimeSchemaMarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// You can implement this trait manually if the schema of your marker type is only
/// known at runtime. If the schema is known at compile time, you'll want to implement
/// [`StaticSchemaMarker`] instead - there is a blanket impl which implements [`Marker`]
/// for any type that implements [`StaticSchemaMarker`].
pub trait Marker {
    /// The [`MarkerTypeHandle`] of this marker type. Created with [`Profile::register_marker_type`] or
    /// with [`Profile::static_schema_marker_type`].
    fn marker_type(&self, profile: &mut Profile) -> MarkerTypeHandle;

    /// The name of this marker, as an interned string handle.
    ///
    /// The name is shown as the row label in the marker chart. It can also be
    /// used as `{marker.name}` in the various `label` template strings in the schema.
    fn name(&self, profile: &mut Profile) -> StringHandle;

    /// The category of this marker. The marker chart groups marker rows by category.
    fn category(&self, profile: &mut Profile) -> CategoryHandle;

    /// Called for any fields defined in the schema whose [`format`](RuntimeSchemaMarkerField::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::String`].
    ///
    /// `field_index` is an index into the schema's [`fields`](RuntimeSchemaMarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any string fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`RuntimeSchemaMarkerSchema::type_name`].
    fn string_field_value(&self, field_index: u32) -> StringHandle;

    /// Called for any fields defined in the schema whose [`format`](RuntimeSchemaMarkerField::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::Number`].
    ///
    /// `field_index` is an index into the schema's [`fields`](RuntimeSchemaMarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any number fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`RuntimeSchemaMarkerSchema::type_name`].
    fn number_field_value(&self, field_index: u32) -> f64;
}

/// The trait for markers whose schema is known at compile time. Any type which implements
/// [`StaticSchemaMarker`] automatically implements the [`Marker`] trait via a blanket impl.
///
/// Markers have a type, a name, a category, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`RuntimeSchemaMarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// In [`StaticSchemaMarker`], the schema is returned from a static `schema` method.
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocations, MarkerFieldFlags, MarkerFieldFormat, StaticSchemaMarkerField,
///     StaticSchemaMarker, CategoryHandle, StringHandle,
/// };
///
/// /// An example marker type with a name and some text content.
/// #[derive(Debug, Clone)]
/// pub struct TextMarker {
///   pub name: StringHandle,
///   pub text: StringHandle,
/// }
///
/// impl StaticSchemaMarker for TextMarker {
///     const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
///
///     const LOCATIONS: MarkerLocations = MarkerLocations::MARKER_CHART.union(MarkerLocations::MARKER_TABLE);
///     const CHART_LABEL: Option<&'static str> = Some("{marker.data.text}");
///     const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.text}");
///
///     const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
///         key: "text",
///         label: "Contents",
///         format: MarkerFieldFormat::String,
///         flags: MarkerFieldFlags::SEARCHABLE,
///     }];
///
///     fn name(&self, _profile: &mut Profile) -> StringHandle {
///         self.name
///     }
///
///     fn category(&self, _profile: &mut Profile) -> CategoryHandle {
///         CategoryHandle::OTHER
///     }
///
///     fn string_field_value(&self, _field_index: u32) -> StringHandle {
///         self.text
///     }
///
///     fn number_field_value(&self, _field_index: u32) -> f64 {
///         unreachable!()
///     }
/// }
/// ```
pub trait StaticSchemaMarker {
    /// A unique string name for this marker type. Has to match the
    /// [`RuntimeSchemaMarkerSchema::type_name`] of this type's schema.
    const UNIQUE_MARKER_TYPE_NAME: &'static str;

    /// An optional description string. Applies to all markers of this type.
    const DESCRIPTION: Option<&'static str> = None;

    /// Set of marker display locations.
    const LOCATIONS: MarkerLocations =
        MarkerLocations::MARKER_CHART.union(MarkerLocations::MARKER_TABLE);

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// If set to `None`, the boxes in the marker chart will be empty.
    const CHART_LABEL: Option<&'static str> = None;

    /// A template string defining the label shown in the first row of the marker's tooltip.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`.
    const TOOLTIP_LABEL: Option<&'static str> = None;

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldkey}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`.
    const TABLE_LABEL: Option<&'static str> = None;

    /// The marker fields. The values are supplied by each marker, in the marker's
    /// implementations of the `string_field_value` and `number_field_value` trait methods.
    const FIELDS: &'static [StaticSchemaMarkerField];

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
    const GRAPHS: &'static [StaticSchemaMarkerGraph] = &[];

    /// The name of this marker, as an interned string handle.
    ///
    /// The name is shown as the row label in the marker chart. It can also be
    /// used as `{marker.name}` in the various `label` template strings in the schema.
    fn name(&self, profile: &mut Profile) -> StringHandle;

    /// The category of this marker. The marker chart groups marker rows by category.
    fn category(&self, profile: &mut Profile) -> CategoryHandle;

    /// Called for any fields defined in the schema whose [`format`](RuntimeSchemaMarkerField::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::String`].
    ///
    /// `field_index` is an index into the schema's [`fields`](RuntimeSchemaMarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any string fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`RuntimeSchemaMarkerSchema::type_name`].
    fn string_field_value(&self, field_index: u32) -> StringHandle;

    /// Called for any fields defined in the schema whose [`format`](RuntimeSchemaMarkerField::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::Number`].
    ///
    /// `field_index` is an index into the schema's [`fields`](RuntimeSchemaMarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any number fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`RuntimeSchemaMarkerSchema::type_name`].
    fn number_field_value(&self, field_index: u32) -> f64;
}

impl<T: StaticSchemaMarker> Marker for T {
    fn marker_type(&self, profile: &mut Profile) -> MarkerTypeHandle {
        profile.static_schema_marker_type::<Self>()
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        <T as StaticSchemaMarker>::name(self, profile)
    }

    fn category(&self, profile: &mut Profile) -> CategoryHandle {
        <T as StaticSchemaMarker>::category(self, profile)
    }

    fn string_field_value(&self, field_index: u32) -> StringHandle {
        <T as StaticSchemaMarker>::string_field_value(self, field_index)
    }

    fn number_field_value(&self, field_index: u32) -> f64 {
        <T as StaticSchemaMarker>::number_field_value(self, field_index)
    }
}

/// Describes a marker type, including the names and types of the marker's fields.
/// You only need this if you don't know the schema until runtime. Otherwise, use
/// [`StaticSchemaMarker`] instead.
///
/// Example:
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocations, MarkerFieldFlags, MarkerFieldFormat, RuntimeSchemaMarkerSchema, RuntimeSchemaMarkerField,
///     CategoryHandle, StringHandle,
/// };
///
/// # fn fun() {
/// let schema = RuntimeSchemaMarkerSchema {
///     type_name: "custom".into(),
///     locations: MarkerLocations::MARKER_CHART | MarkerLocations::MARKER_TABLE,
///     chart_label: Some("{marker.data.eventName}".into()),
///     tooltip_label: Some("Custom {marker.name} marker".into()),
///     table_label: Some("{marker.name} - {marker.data.eventName} with allocation size {marker.data.allocationSize} (latency: {marker.data.latency})".into()),
///     fields: vec![
///         RuntimeSchemaMarkerField {
///             key: "eventName".into(),
///             label: "Event name".into(),
///             format: MarkerFieldFormat::String,
///             flags: MarkerFieldFlags::SEARCHABLE,
///         },
///         RuntimeSchemaMarkerField {
///             key: "allocationSize".into(),
///             label: "Allocation size".into(),
///             format: MarkerFieldFormat::Bytes,
///             flags: MarkerFieldFlags::SEARCHABLE,
///         },
///         RuntimeSchemaMarkerField {
///             key: "url".into(),
///             label: "URL".into(),
///             format: MarkerFieldFormat::Url,
///             flags: MarkerFieldFlags::SEARCHABLE,
///         },
///         RuntimeSchemaMarkerField {
///             key: "latency".into(),
///             label: "Latency".into(),
///             format: MarkerFieldFormat::Duration,
///             flags: MarkerFieldFlags::SEARCHABLE,
///         },
///     ],
///     description: Some("This is a test marker with a custom schema.".into()),
///     graphs: vec![],
/// };
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RuntimeSchemaMarkerSchema {
    /// The unique name of this marker type. There must not be any other schema
    /// with the same name.
    pub type_name: String,

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
    /// implementations of the `string_field_value` and `number_field_value` trait methods.
    pub fields: Vec<RuntimeSchemaMarkerField>,

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
    pub graphs: Vec<RuntimeSchemaMarkerGraph>,
}

bitflags! {
    /// Locations in the profiler UI where markers can be displayed.
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
    pub struct MarkerLocations: u32 {
        /// Show the marker in the "marker chart" panel.
        const MARKER_CHART = 1 << 0;
        /// Show the marker in the marker table.
        const MARKER_TABLE = 1 << 1;
        /// This adds markers to the main marker timeline in the header, but only
        /// for main threads and for threads that were specifically asked to show
        /// these markers using [`Profile::set_thread_show_markers_in_timeline`].
        const TIMELINE_OVERVIEW = 1 << 2;
        /// In the timeline, this is a section that breaks out markers that are
        /// related to memory. When memory counters are used, this is its own
        /// track, otherwise it is displayed with the main thread.
        const TIMELINE_MEMORY = 1 << 3;
        /// This adds markers to the IPC timeline area in the header.
        const TIMELINE_IPC = 1 << 4;
        /// This adds markers to the FileIO timeline area in the header.
        const TIMELINE_FILEIO = 1 << 5;
    }
}

/// The field definition of a marker field, used in [`StaticSchemaMarker::FIELDS`].
///
/// For each marker which uses this schema, the value for this field is supplied by the
/// marker's implementation of [`number_field_value`](Marker::number_field_value) /
/// [`string_field_value`](Marker::string_field_value), depending on this field
/// format's [kind](MarkerFieldFormat::kind).
///
/// Used with runtime-generated marker schemas. Use [`RuntimeSchemaMarkerField`]
/// when using [`RuntimeSchemaMarkerSchema`].
pub struct StaticSchemaMarkerField {
    /// The field key. Must not be `type` or `cause`.
    pub key: &'static str,

    /// The user-visible label of this field.
    pub label: &'static str,

    /// The format of this field.
    pub format: MarkerFieldFormat,

    /// Additional field flags.
    pub flags: MarkerFieldFlags,
}

/// The field definition of a marker field, used in [`RuntimeSchemaMarkerSchema::fields`].
///
/// For each marker which uses this schema, the value for this field is supplied by the
/// marker's implementation of [`number_field_value`](Marker::number_field_value) /
/// [`string_field_value`](Marker::string_field_value), depending on this field
/// format's [kind](MarkerFieldFormat::kind).
///
/// Used with runtime-generated marker schemas. Use [`StaticSchemaMarkerField`]
/// when using [`StaticSchemaMarker`].
#[derive(Debug, Clone)]
pub struct RuntimeSchemaMarkerField {
    /// The field key. Must not be `type` or `cause`.
    pub key: String,

    /// The user-visible label of this field.
    pub label: String,

    /// The format of this field.
    pub format: MarkerFieldFormat,

    /// Whether this field's value should be matched against search terms.
    pub flags: MarkerFieldFlags,
}

impl From<&StaticSchemaMarkerField> for RuntimeSchemaMarkerField {
    fn from(schema: &StaticSchemaMarkerField) -> Self {
        Self {
            key: schema.key.into(),
            label: schema.label.into(),
            format: schema.format.clone(),
            flags: schema.flags,
        }
    }
}

/// The field format of a marker field.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerFieldFormat {
    // ----------------------------------------------------
    // String types.
    /// A URL, supports PII sanitization
    Url,

    /// A file path, supports PII sanitization.
    FilePath,

    /// A regular string, supports PII sanitization.
    /// Concretely this means that these strings are stripped when uploading
    /// profiles if you uncheck "Include resource URLs and paths".
    SanitizedString,

    /// A plain String, never sanitized for PII.
    ///
    /// Important: Do not put URL or file path information here, as it will not
    /// be sanitized during profile upload. Please be careful with including
    /// other types of PII here as well.
    #[serde(rename = "unique-string")]
    String,

    // ----------------------------------------------------
    // Numeric types
    /// For time data that represents a duration of time.
    /// The value is given in float milliseconds and will be displayed
    /// in a unit that is picked based on the magnitude of the number.
    /// e.g. "Label: 5s, 5ms, 5μs"
    Duration,

    /// A timestamp, relative to the start of the profile. The value is given in
    /// float milliseconds.
    ///
    ///  e.g. "Label: 15.5s, 20.5ms, 30.5μs"
    Time,

    /// Display a millisecond value as seconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5s" for a value of 5000.0
    Seconds,

    /// Display a millisecond value as milliseconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5ms" for a value of 5.0
    Milliseconds,

    /// Display a millisecond value as microseconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5μs" for a value of 0.0005
    Microseconds,

    /// Display a millisecond value as seconds, regardless of the magnitude of the number.
    ///
    /// e.g. "Label: 5ns" for a value of 0.0000005
    Nanoseconds,

    /// Display a bytes value in a unit that's appropriate for the number's magnitude.
    ///
    /// e.g. "Label: 5.55mb, 5 bytes, 312.5kb"
    Bytes,

    /// This should be a value between 0 and 1.
    /// e.g. "Label: 50%" for a value of 0.5
    Percentage,

    /// A generic integer number.
    /// Do not use it for time information.
    ///
    /// "Label: 52, 5,323, 1,234,567"
    Integer,

    /// A generic floating point number.
    /// Do not use it for time information.
    ///
    /// "Label: 52.23, 0.0054, 123,456.78"
    Decimal,
}

/// The kind of a marker field. Every marker field is either a string or a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerFieldFormatKind {
    String,
    Number,
}

impl MarkerFieldFormat {
    /// Whether this field is a number field or a string field.
    ///
    /// This determines whether we call `number_field_value` or
    /// `string_field_value` to get the field values.
    pub fn kind(&self) -> MarkerFieldFormatKind {
        match self {
            Self::Url | Self::FilePath | Self::SanitizedString | Self::String => {
                MarkerFieldFormatKind::String
            }
            Self::Duration
            | Self::Time
            | Self::Seconds
            | Self::Milliseconds
            | Self::Microseconds
            | Self::Nanoseconds
            | Self::Bytes
            | Self::Percentage
            | Self::Integer
            | Self::Decimal => MarkerFieldFormatKind::Number,
        }
    }
}

bitflags! {
    /// Marker field flags, used in the marker schema.
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
    pub struct MarkerFieldFlags: u32 {
        /// Whether this field's value should be matched against search terms.
        const SEARCHABLE = 0b00000001;
    }
}

/// A graph within a marker graph track, used in [`StaticSchemaMarker::GRAPHS`].
///
/// Used with runtime-generated marker schemas. Use [`RuntimeSchemaMarkerGraph`]
/// when using [`RuntimeSchemaMarkerSchema`].
pub struct StaticSchemaMarkerGraph {
    /// The key of a number field that's declared in the marker schema.
    ///
    /// The values of this field are the values of this graph line /
    /// bar graph segment.
    pub key: &'static str,
    /// Whether this marker graph segment is a line or a bar graph segment.
    pub graph_type: MarkerGraphType,
    /// The color of the graph segment. If `None`, the choice is up to the front-end.
    pub color: Option<GraphColor>,
}

/// A graph within a marker graph track, used in [`RuntimeSchemaMarkerSchema::graphs`].
///
/// Used with runtime-generated marker schemas. Use [`StaticSchemaMarkerGraph`]
/// when using [`StaticSchemaMarker`].
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeSchemaMarkerGraph {
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

impl From<&StaticSchemaMarkerGraph> for RuntimeSchemaMarkerGraph {
    fn from(schema: &StaticSchemaMarkerGraph) -> Self {
        Self {
            key: schema.key.into(),
            graph_type: schema.graph_type,
            color: schema.color,
        }
    }
}

/// The type of a graph segment within a marker graph.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerGraphType {
    /// As a bar graph.
    Bar,
    /// As lines.
    Line,
    /// As lines that are colored underneath.
    LineFilled,
}

/// The color used for a graph segment within a marker graph.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphColor {
    Blue,
    Green,
    Grey,
    Ink,
    Magenta,
    Orange,
    Purple,
    Red,
    Teal,
    Yellow,
}

#[derive(Debug, Clone)]
pub struct InternalMarkerSchema {
    /// The name of this marker type.
    type_name: String,

    /// List of marker display locations.
    locations: MarkerLocations,

    chart_label: Option<String>,
    tooltip_label: Option<String>,
    table_label: Option<String>,

    /// The marker fields. These can be specified on each marker.
    fields: Vec<RuntimeSchemaMarkerField>,

    /// Any graph tracks created from markers of this type
    graphs: Vec<RuntimeSchemaMarkerGraph>,

    string_field_count: usize,
    number_field_count: usize,

    description: Option<String>,
}

impl From<RuntimeSchemaMarkerSchema> for InternalMarkerSchema {
    fn from(schema: RuntimeSchemaMarkerSchema) -> Self {
        Self::from_runtime_schema(schema)
    }
}

impl InternalMarkerSchema {
    pub fn from_runtime_schema(schema: RuntimeSchemaMarkerSchema) -> Self {
        let string_field_count = schema
            .fields
            .iter()
            .filter(|f| f.format.kind() == MarkerFieldFormatKind::String)
            .count();
        let number_field_count = schema
            .fields
            .iter()
            .filter(|f| f.format.kind() == MarkerFieldFormatKind::Number)
            .count();
        Self {
            type_name: schema.type_name,
            locations: schema.locations,
            chart_label: schema.chart_label,
            tooltip_label: schema.tooltip_label,
            table_label: schema.table_label,
            fields: schema.fields,
            graphs: schema.graphs,
            string_field_count,
            number_field_count,
            description: schema.description,
        }
    }

    pub fn from_static_schema<T: StaticSchemaMarker>() -> Self {
        let string_field_count = T::FIELDS
            .iter()
            .filter(|f| f.format.kind() == MarkerFieldFormatKind::String)
            .count();
        let number_field_count = T::FIELDS
            .iter()
            .filter(|f| f.format.kind() == MarkerFieldFormatKind::Number)
            .count();
        Self {
            type_name: T::UNIQUE_MARKER_TYPE_NAME.into(),
            locations: T::LOCATIONS,
            chart_label: T::CHART_LABEL.map(Into::into),
            tooltip_label: T::TOOLTIP_LABEL.map(Into::into),
            table_label: T::TABLE_LABEL.map(Into::into),
            fields: T::FIELDS.iter().map(Into::into).collect(),
            string_field_count,
            number_field_count,
            description: T::DESCRIPTION.map(Into::into),
            graphs: T::GRAPHS.iter().map(Into::into).collect(),
        }
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }
    pub fn fields(&self) -> &[RuntimeSchemaMarkerField] {
        &self.fields
    }
    pub fn string_field_count(&self) -> usize {
        self.string_field_count
    }
    pub fn number_field_count(&self) -> usize {
        self.number_field_count
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

struct SerializableSchemaField<'a>(&'a RuntimeSchemaMarkerField);

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
        if self.0.flags.contains(MarkerFieldFlags::SEARCHABLE) {
            map.serialize_entry("searchable", &true)?;
        }
        map.end()
    }
}

struct SerializableSchemaDisplay(MarkerLocations);

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
