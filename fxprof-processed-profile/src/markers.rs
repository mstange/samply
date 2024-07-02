/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this file,
 * You can obtain one at http://mozilla.org/MPL/2.0/. */

use serde::ser::{SerializeMap, SerializeSeq};
use serde::Serialize;
use serde_derive::Serialize;

use crate::{CategoryHandle, Profile};

use super::profile::StringHandle;
use super::timestamp::Timestamp;

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
/// The fields of a marker type are defined by the marker type's schema, see [`MarkerSchema`].
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

    /// Called for any fields defined in the schema whose [`format`](MarkerFieldSchema::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::String`].
    ///
    /// `field_index` is an index into the schema's [`fields`](MarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any string fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`MarkerSchema::type_name`].
    fn string_field_value(&self, field_index: u32) -> StringHandle;

    /// Called for any fields defined in the schema whose [`format`](MarkerFieldSchema::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::Number`].
    ///
    /// `field_index` is an index into the schema's [`fields`](MarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any number fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`MarkerSchema::type_name`].
    fn number_field_value(&self, field_index: u32) -> f64;
}

/// The trait for markers whose schema is known at compile time. Any type which implements
/// [`StaticSchemaMarker`] automatically implements the [`Marker`] trait via a blanket impl.
///
/// Markers have a type, a name, a category, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`MarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// In [`StaticSchemaMarker`], the schema is returned from a static `schema` method.
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocation, MarkerFieldFormat, MarkerSchema, MarkerFieldSchema,
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
///     fn schema() -> MarkerSchema {
///         MarkerSchema {
///             type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
///             locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
///             chart_label: Some("{marker.data.text}".into()),
///             tooltip_label: None,
///             table_label: Some("{marker.name} - {marker.data.text}".into()),
///             fields: vec![MarkerFieldSchema {
///                 key: "text".into(),
///                 label: "Contents".into(),
///                 format: MarkerFieldFormat::String,
///                 searchable: true,
///             }],
///             static_fields: vec![],
///         }
///     }
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
    /// [`MarkerSchema::type_name`] of this type's schema.
    const UNIQUE_MARKER_TYPE_NAME: &'static str;

    /// The [`MarkerSchema`] for this marker type.
    fn schema() -> MarkerSchema;

    /// The name of this marker, as an interned string handle.
    ///
    /// The name is shown as the row label in the marker chart. It can also be
    /// used as `{marker.name}` in the various `label` template strings in the schema.
    fn name(&self, profile: &mut Profile) -> StringHandle;

    /// The category of this marker. The marker chart groups marker rows by category.
    fn category(&self, profile: &mut Profile) -> CategoryHandle;

    /// Called for any fields defined in the schema whose [`format`](MarkerFieldSchema::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::String`].
    ///
    /// `field_index` is an index into the schema's [`fields`](MarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any string fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`MarkerSchema::type_name`].
    fn string_field_value(&self, field_index: u32) -> StringHandle;

    /// Called for any fields defined in the schema whose [`format`](MarkerFieldSchema::format) is
    /// of [kind](MarkerFieldFormat::kind) [`MarkerFieldFormatKind::Number`].
    ///
    /// `field_index` is an index into the schema's [`fields`](MarkerSchema::fields).
    ///
    /// You can panic for any unexpected field indexes, for example
    /// using `unreachable!()`. You can even panic unconditionally if this
    /// marker type doesn't have any number fields.
    ///
    /// If you do see unexpected calls to this method, make sure you're not registering
    /// multiple different schemas with the same [`MarkerSchema::type_name`].
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
///
/// Example:
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocation, MarkerFieldFormat, MarkerSchema, MarkerFieldSchema,
///     MarkerStaticField, StaticSchemaMarker, CategoryHandle, StringHandle,
/// };
///
/// # fn fun() {
/// let schema = MarkerSchema {
///     type_name: "custom".into(),
///     locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
///     chart_label: Some("{marker.data.eventName}".into()),
///     tooltip_label: Some("Custom {marker.name} marker".into()),
///     table_label: Some("{marker.name} - {marker.data.eventName} with allocation size {marker.data.allocationSize} (latency: {marker.data.latency})".into()),
///     fields: vec![
///         MarkerFieldSchema {
///             key: "eventName".into(),
///             label: "Event name".into(),
///             format: MarkerFieldFormat::String,
///             searchable: true,
///         },
///         MarkerFieldSchema {
///             key: "allocationSize".into(),
///             label: "Allocation size".into(),
///             format: MarkerFieldFormat::Bytes,
///             searchable: true,
///         },
///         MarkerFieldSchema {
///             key: "url".into(),
///             label: "URL".into(),
///             format: MarkerFieldFormat::Url,
///             searchable: true,
///         },
///         MarkerFieldSchema {
///             key: "latency".into(),
///             label: "Latency".into(),
///             format: MarkerFieldFormat::Duration,
///             searchable: true,
///         },
///     ],
///     static_fields: vec![MarkerStaticField {
///         label: "Description".into(),
///         value: "This is a test marker with a custom schema.".into(),
///     }],
/// };
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MarkerSchema {
    /// The unique name of this marker type. There must not be any other schema
    /// with the same name.
    pub type_name: String,

    /// List of marker display locations.
    pub locations: Vec<MarkerLocation>,

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldname}`.
    ///
    /// If set to `None`, the boxes in the marker chart will be empty.
    pub chart_label: Option<String>,

    /// A template string defining the label shown in the first row of the marker's tooltip.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldname}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`. (TODO: verify this is true)
    pub tooltip_label: Option<String>,

    /// A template string defining the label shown within each marker's box in the marker chart.
    ///
    /// Usable template literals are `{marker.name}` and `{marker.data.fieldname}`.
    ///
    /// Defaults to `{marker.name}` if set to `None`. (TODO: verify this is true)
    pub table_label: Option<String>,

    /// The marker fields. The values are supplied by each marker, in the marker's
    /// implementations of the `string_field_value` and `number_field_value` trait methods.
    pub fields: Vec<MarkerFieldSchema>,

    /// The static fields of this marker type, with fixed values that apply to all markers of this type.
    /// These are usually used for things like a human readable marker type description.
    pub static_fields: Vec<MarkerStaticField>,
}

#[derive(Debug, Clone)]
pub struct InternalMarkerSchema {
    /// The name of this marker type.
    type_name: String,

    /// List of marker display locations.
    locations: Vec<MarkerLocation>,

    chart_label: Option<String>,
    tooltip_label: Option<String>,
    table_label: Option<String>,

    /// The marker fields. These can be specified on each marker.
    fields: Vec<MarkerFieldSchema>,

    string_field_count: usize,
    number_field_count: usize,

    /// The static fields of this marker type, with fixed values that apply to all markers.
    /// These are usually used for things like a human readable marker type description.
    static_fields: Vec<MarkerStaticField>,
}

impl From<MarkerSchema> for InternalMarkerSchema {
    fn from(schema: MarkerSchema) -> Self {
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
            string_field_count,
            number_field_count,
            static_fields: schema.static_fields,
        }
    }
}

impl InternalMarkerSchema {
    pub fn type_name(&self) -> &str {
        &self.type_name
    }
    pub fn fields(&self) -> &[MarkerFieldSchema] {
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
        map.serialize_entry("display", &self.locations)?;
        if let Some(label) = &self.chart_label {
            map.serialize_entry("chartLabel", label)?;
        }
        if let Some(label) = &self.tooltip_label {
            map.serialize_entry("tooltipLabel", label)?;
        }
        if let Some(label) = &self.table_label {
            map.serialize_entry("tableLabel", label)?;
        }
        map.serialize_entry("data", &SerializableSchemaFields(self))?;
        map.end()
    }

    fn serialize_fields<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq =
            serializer.serialize_seq(Some(self.fields.len() + self.static_fields.len()))?;
        for field in &self.fields {
            seq.serialize_element(field)?;
        }
        for field in &self.static_fields {
            seq.serialize_element(field)?;
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

impl<'a> Serialize for SerializableSchemaFields<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize_fields(serializer)
    }
}

// /// The location of markers with this type.
///
/// Markers can be shown in different parts of the Firefox Profiler UI.
///
/// Multiple [`MarkerLocation`]s can be specified for a single marker type.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerLocation {
    MarkerChart,
    MarkerTable,
    /// This adds markers to the main marker timeline in the header.
    TimelineOverview,
    /// In the timeline, this is a section that breaks out markers that are
    /// related to memory. When memory counters are enabled, this is its own
    /// track, otherwise it is displayed with the main thread.
    TimelineMemory,
    /// This adds markers to the IPC timeline area in the header.
    TimelineIPC,
    /// This adds markers to the FileIO timeline area in the header.
    #[serde(rename = "timeline-fileio")]
    TimelineFileIO,
    /// TODO - This is not supported yet.
    StackChart,
}

/// The field description of a marker field which has the same key and value on all markers with this schema.
#[derive(Debug, Clone, Serialize)]
pub struct MarkerStaticField {
    pub label: String,
    pub value: String,
}

/// The field description of a marker field. The value for this field is supplied by the marker's implementation
/// of [`number_field_value`](Marker::number_field_value) / [`string_field_value`](Marker::string_field_value).
#[derive(Debug, Clone, Serialize)]
pub struct MarkerFieldSchema {
    /// The field key. Must not be `type` or `cause`.
    pub key: String,

    /// The user-visible label of this field.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub label: String,

    /// The format of this field.
    pub format: MarkerFieldFormat,

    /// Whether this field's value should be matched against search terms.
    pub searchable: bool,
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
