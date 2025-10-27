/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this file,
 * You can obtain one at http://mozilla.org/MPL/2.0/. */

use bitflags::bitflags;
use serde::ser::{Serialize, SerializeMap, SerializeSeq};
use serde_derive::Serialize;

use super::string_table::StringHandle;
use super::timestamp::Timestamp;
use crate::{Category, CategoryHandle, Profile};

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

/// The trait for markers. Implementing [`Marker`] will give you an implementation
/// of [`DynamicSchemaMarker`] via a blanket impl.
///
/// Markers have a type, a name, a category, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`DynamicSchemaMarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// You can implement this trait manually if the schema of your marker type is only
/// known at runtime. If the schema is known at compile time, you'll want to implement
/// [`Marker`] instead - there is a blanket impl which implements [`DynamicSchemaMarker`]
/// for any type that implements [`Marker`].
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

/// The trait that needs to be implemented by the return type of
/// [`Marker::field_values`].
///
/// This trait is implemented for the field value types `f64`, [`StringHandle`]
/// and [`FlowId`], and for tuples of these types.
pub trait MarkerFieldsTrait {
    type Schema;

    const FIELD_KIND_COUNTS: MarkerFieldKindCounts;

    fn push_field_values(self, consumer: &mut impl MarkerFieldValueConsumer);

    fn to_runtime_field_schema(schema: &Self::Schema) -> Vec<DynamicSchemaMarkerField>;
}

/// The trait for markers whose schema is known at compile time. Any type which implements
/// [`Marker`] automatically implements the [`Marker`] trait via a blanket impl.
///
/// Markers have a type, a name, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`DynamicSchemaMarkerSchema`].
/// The timestamps are not part of the marker; they are supplied separately to
/// [`Profile::add_marker`] when a marker is added to the profile.
///
/// ```
/// use fxprof_processed_profile::{
///     Profile, Marker, MarkerLocations, MarkerStringFieldFormat, Schema,
///     MarkerField, Marker, Category, CategoryColor, StringHandle,
/// };
///
/// /// An example marker type with a name and some text content.
/// #[derive(Debug, Clone)]
/// pub struct TextMarker {
///   pub name: StringHandle,
///   pub text: StringHandle,
/// }
///
/// impl Marker for TextMarker {
///     type FieldsType = StringHandle; // alternative: `(StringHandle, ...)` tuple
///
///     const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
///
///     const CATEGORY: Category<'static> = Category("Navigation", CategoryColor::Green);
///
///     const LOCATIONS: MarkerLocations = MarkerLocations::MARKER_CHART.union(MarkerLocations::MARKER_TABLE);
///     const CHART_LABEL: Option<&'static str> = Some("{marker.data.text}");
///     const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.text}");
///
///     const FIELDS: Schema<Self::FieldsType> = Schema(MarkerField::string(
///         "text",
///         "Contents",
///     ));
///
///     fn name(&self, _profile: &mut Profile) -> StringHandle {
///         self.name
///     }
///
///     fn field_values(&self) -> StringHandle {
///         self.text
///     }
/// }
/// ```
pub trait Marker {
    /// A unique string name for this marker type. Has to match the
    /// [`DynamicSchemaMarkerSchema::type_name`] of this type's schema.
    const UNIQUE_MARKER_TYPE_NAME: &'static str;

    /// The category of this marker. The marker chart groups marker rows by category.
    const CATEGORY: Category<'static> = Category::OTHER;

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

    /// The type returned by `field_values`.
    ///
    /// Individual fields are either numbers (`f64`), strings ([`StringHandle`]), or
    /// flows ([`FlowId`]). You can use one of those types, or a tuple of them.
    type FieldsType: MarkerFieldsTrait;

    /// The marker fields. The values are supplied by each marker, in the marker's
    /// implementations of the `string_field_value` and `number_field_value` trait methods.
    const FIELDS: Schema<Self::FieldsType>;

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
    const GRAPHS: &'static [MarkerGraph] = &[];

    /// The name of this marker, as an interned string handle.
    ///
    /// The name is shown as the row label in the marker chart. It can also be
    /// used as `{marker.name}` in the various `label` template strings in the schema.
    fn name(&self, profile: &mut Profile) -> StringHandle;

    /// Returns the values for the fields of this marker.
    fn field_values(&self) -> Self::FieldsType;
}

impl<T: Marker> DynamicSchemaMarker for T {
    fn marker_type(&self, profile: &mut Profile) -> MarkerTypeHandle {
        profile.static_schema_marker_type::<Self>()
    }

    fn name(&self, profile: &mut Profile) -> StringHandle {
        <T as Marker>::name(self, profile)
    }

    fn push_field_values(&self, consumer: &mut impl MarkerFieldValueConsumer) {
        let values = self.field_values();
        <<T as Marker>::FieldsType as MarkerFieldsTrait>::push_field_values(values, consumer);
    }
}

/// Describes a marker type, including the names and types of the marker's fields.
/// You only need this if you don't know the schema until runtime. Otherwise, use
/// [`Marker`] instead.
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
    /// implementations of the `string_field_value` and `number_field_value` trait methods.
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

/// The kind of a marker field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MarkerFieldKind {
    String,
    Number,
    Flow,
}

/// Used in [`MarkerFieldsTrait::FIELD_KIND_COUNTS`].
#[derive(Default, Debug, Clone)]
pub struct MarkerFieldKindCounts {
    pub string_field_count: usize,
    pub number_field_count: usize,
    pub flow_field_count: usize,
}

impl MarkerFieldKindCounts {
    pub const fn new() -> Self {
        Self {
            string_field_count: 0,
            number_field_count: 0,
            flow_field_count: 0,
        }
    }

    pub const fn add(&mut self, kind: MarkerFieldKind) {
        match kind {
            MarkerFieldKind::String => self.string_field_count += 1,
            MarkerFieldKind::Number => self.number_field_count += 1,
            MarkerFieldKind::Flow => self.flow_field_count += 1,
        }
    }

    pub const fn from_kind(kind: MarkerFieldKind) -> Self {
        let mut counts = Self::new();
        counts.add(kind);
        counts
    }

    pub const fn from_kinds(kinds: &[MarkerFieldKind]) -> Self {
        let mut counts = Self::new();
        let mut i = 0;
        let len = kinds.len();
        while i < len {
            counts.add(kinds[i]);
            i += 1;
        }
        counts
    }
}

/// The trait for types that can be used for marker field values.
pub trait MarkerFieldValueType {
    type FormatEnum: Clone + core::fmt::Debug + Into<DynamicSchemaMarkerFieldFormat>;
    const KIND: MarkerFieldKind;
    fn push_field_value(self, consumer: &mut impl MarkerFieldValueConsumer);
}

impl MarkerFieldValueType for StringHandle {
    type FormatEnum = MarkerStringFieldFormat;
    const KIND: MarkerFieldKind = MarkerFieldKind::String;
    fn push_field_value(self, consumer: &mut impl MarkerFieldValueConsumer) {
        consumer.consume_string_field(self);
    }
}

impl MarkerFieldValueType for f64 {
    type FormatEnum = MarkerNumberFieldFormat;
    const KIND: MarkerFieldKind = MarkerFieldKind::Number;
    fn push_field_value(self, consumer: &mut impl MarkerFieldValueConsumer) {
        consumer.consume_number_field(self);
    }
}

/// An 64-bit ID which identifies a "flow".
///
/// Markers with shared flows are connected in the UI. A "flow" can represent
/// any kind of entity, such as an IPC message, a task, a network request,
/// a generic heap-allocated object, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FlowId(pub u64);

impl MarkerFieldValueType for FlowId {
    type FormatEnum = MarkerFlowFieldFormat;
    const KIND: MarkerFieldKind = MarkerFieldKind::Flow;
    fn push_field_value(self, consumer: &mut impl MarkerFieldValueConsumer) {
        consumer.consume_flow_field(self.0);
    }
}

impl MarkerFieldsTrait for () {
    type Schema = ();

    const FIELD_KIND_COUNTS: MarkerFieldKindCounts = MarkerFieldKindCounts::new();

    fn push_field_values(self, _consumer: &mut impl MarkerFieldValueConsumer) {
        // No fields to push
    }

    fn to_runtime_field_schema(_schema: &Self::Schema) -> Vec<DynamicSchemaMarkerField> {
        vec![]
    }
}

impl<T: MarkerFieldValueType> MarkerFieldsTrait for T {
    type Schema = MarkerField<T>;

    const FIELD_KIND_COUNTS: MarkerFieldKindCounts = MarkerFieldKindCounts::from_kind(Self::KIND);

    fn push_field_values(self, consumer: &mut impl MarkerFieldValueConsumer) {
        Self::push_field_value(self, consumer);
    }

    fn to_runtime_field_schema(schema: &Self::Schema) -> Vec<DynamicSchemaMarkerField> {
        vec![DynamicSchemaMarkerField::from(schema)]
    }
}

// Macro to generate MarkerFieldsTrait implementations for tuples
macro_rules! impl_marker_fields_for_tuples {
    ($(($($T:ident),+)),+) => {
        $(
            #[allow(non_snake_case)]
            impl<$($T),+> MarkerFieldsTrait for ($($T,)+)
            where
                $($T: MarkerFieldValueType,)+
            {
                type Schema = ($(MarkerField<$T>,)+);

                const FIELD_KIND_COUNTS: MarkerFieldKindCounts =
                    MarkerFieldKindCounts::from_kinds(&[$($T::KIND,)+]);

                #[allow(non_snake_case)]
                fn push_field_values(self, consumer: &mut impl MarkerFieldValueConsumer) {
                    let ($($T,)+) = self;
                    $(
                        $T::push_field_value($T, consumer);
                    )+
                }

                fn to_runtime_field_schema(schema: &Self::Schema) -> Vec<DynamicSchemaMarkerField> {
                    let ($($T,)+) = schema;
                    vec![$(DynamicSchemaMarkerField::from($T),)+]
                }
            }
        )+
    };
}

// Generate implementations for tuples up to length 16
impl_marker_fields_for_tuples! {
    (T0),
    (T0, T1),
    (T0, T1, T2),
    (T0, T1, T2, T3),
    (T0, T1, T2, T3, T4),
    (T0, T1, T2, T3, T4, T5),
    (T0, T1, T2, T3, T4, T5, T6),
    (T0, T1, T2, T3, T4, T5, T6, T7),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14),
    (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15)
}

/// The field definition of a marker field, used in [`Marker::FIELDS`].
///
/// For each marker which uses this schema, the value for this field is supplied by the
/// marker's implementation of [`number_field_value`](Marker::number_field_value) /
/// [`string_field_value`](Marker::string_field_value), depending on this field
/// format's [kind](MarkerFieldFormat::kind).
///
/// Used with static marker schemas. Use [`DynamicSchemaMarkerField`]
/// when using [`DynamicSchemaMarkerSchema`].
pub struct MarkerField<T: MarkerFieldValueType> {
    /// The field key. Must not be `type` or `cause`.
    key: &'static str,

    /// The user-visible label of this field.
    label: &'static str,

    /// The format of this field.
    format: <T as MarkerFieldValueType>::FormatEnum,
}

impl<T: MarkerFieldValueType> MarkerField<T> {
    pub const fn new(
        key: &'static str,
        label: &'static str,
        format: <T as MarkerFieldValueType>::FormatEnum,
    ) -> Self {
        Self { key, label, format }
    }
}

/// Convenience constructors for string fields.
impl MarkerField<StringHandle> {
    /// Creates a string field with the default string format.
    pub const fn string(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerStringFieldFormat::String)
    }

    /// Creates a URL field that will be rendered as a clickable link.
    pub const fn url(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerStringFieldFormat::Url)
    }

    /// Creates a file path field.
    pub const fn file_path(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerStringFieldFormat::FilePath)
    }

    /// Creates a sanitized string field (supports PII sanitization).
    pub const fn sanitized_string(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerStringFieldFormat::SanitizedString)
    }
}

/// Convenience constructors for number fields.
impl MarkerField<f64> {
    /// Creates a field for byte values with appropriate formatting.
    pub const fn bytes(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerNumberFieldFormat::Bytes)
    }

    /// Creates a field for duration values with appropriate formatting.
    pub const fn duration(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerNumberFieldFormat::Duration)
    }

    /// Creates a field for integer values.
    pub const fn integer(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerNumberFieldFormat::Integer)
    }

    /// Creates a field for decimal values.
    pub const fn decimal(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerNumberFieldFormat::Decimal)
    }

    /// Creates a field for percentage values (0.0-1.0 displayed as 0%-100%).
    pub const fn percentage(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerNumberFieldFormat::Percentage)
    }
}

/// Convenience constructors for flow fields.
impl MarkerField<FlowId> {
    /// Creates a flow field for flow connections.
    pub const fn flow(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerFlowFieldFormat::Flow)
    }

    /// Creates a terminating flow field for ending flow connections.
    pub const fn terminating_flow(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, MarkerFlowFieldFormat::TerminatingFlow)
    }
}

/// A wrapper type used in [`Marker::FIELDS`], usually wraps a tuple.
pub struct Schema<FieldsType: MarkerFieldsTrait>(pub FieldsType::Schema);

/// The field definition of a marker field, used in [`DynamicSchemaMarkerSchema::fields`].
///
/// For each marker which uses this schema, the value for this field is supplied by the
/// marker's implementation of [`number_field_value`](Marker::number_field_value) /
/// [`string_field_value`](Marker::string_field_value), depending on this field
/// format's [kind](MarkerFieldFormat::kind).
///
/// Used with runtime-generated marker schemas. Use [`MarkerField`]
/// when using [`Marker`].
#[derive(Debug, Clone)]
pub struct DynamicSchemaMarkerField {
    /// The field key. Must not be `type` or `cause`.
    pub key: String,

    /// The user-visible label of this field.
    pub label: String,

    /// The format of this field.
    pub format: DynamicSchemaMarkerFieldFormat,
}

impl<T: MarkerFieldValueType> From<&MarkerField<T>> for DynamicSchemaMarkerField {
    fn from(schema: &MarkerField<T>) -> Self {
        Self {
            key: schema.key.into(),
            label: schema.label.into(),
            format: schema.format.clone().into(),
        }
    }
}

/// The field format of a marker field of kind [`MarkerFieldKind::String`]`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerStringFieldFormat {
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
}

/// The field format of a marker field of kind [`MarkerFieldKind::Number`]`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerNumberFieldFormat {
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

/// The field format of a marker field of kind [`MarkerFieldKind::Flow`]`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerFlowFieldFormat {
    /// A flow is a u64 identifier that's unique across processes. All of
    /// the markers with same flow id before a terminating flow id will be
    /// considered part of the same "flow" and linked together.
    #[serde(rename = "flow-id")]
    Flow,

    /// A terminating flow ends a flow of a particular id and allows that id
    /// to be reused again. It often makes sense for destructors to create
    /// a marker with a field of this type.
    #[serde(rename = "terminating-flow-id")]
    TerminatingFlow,
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

/// A graph within a marker graph track, used in [`Marker::GRAPHS`].
///
/// Used with static marker schemas. Use [`DynamicSchemaMarkerGraph`]
/// when using [`DynamicSchemaMarkerSchema`].
pub struct MarkerGraph {
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

/// A graph within a marker graph track, used in [`DynamicSchemaMarkerSchema::graphs`].
///
/// Used with runtime-generated marker schemas. Use [`MarkerGraph`]
/// when using [`Marker`].
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

impl From<&MarkerGraph> for DynamicSchemaMarkerGraph {
    fn from(schema: &MarkerGraph) -> Self {
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

pub trait MarkerFieldValueConsumer {
    fn consume_string_field(&mut self, string_handle: StringHandle);
    fn consume_number_field(&mut self, number: f64);
    fn consume_flow_field(&mut self, flow: u64);
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

struct SerializableSchemaField<'a>(&'a DynamicSchemaMarkerField);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tuple_marker_fields_up_to_16() {
        // Test that MarkerFieldsTrait is implemented for tuples up to length 16

        // Test a 1-tuple
        let _: <(StringHandle,) as MarkerFieldsTrait>::Schema;

        // Test a 2-tuple
        let _: <(StringHandle, f64) as MarkerFieldsTrait>::Schema;

        // Test a 16-tuple (largest supported)
        type SixteenTuple = (
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
            StringHandle,
            f64,
        );
        let _: <SixteenTuple as MarkerFieldsTrait>::Schema;

        // Verify field counts work correctly for different tuple sizes
        assert_eq!(
            <(StringHandle,) as MarkerFieldsTrait>::FIELD_KIND_COUNTS.string_field_count,
            1
        );
        assert_eq!(
            <(StringHandle,) as MarkerFieldsTrait>::FIELD_KIND_COUNTS.number_field_count,
            0
        );

        assert_eq!(
            <(StringHandle, f64) as MarkerFieldsTrait>::FIELD_KIND_COUNTS.string_field_count,
            1
        );
        assert_eq!(
            <(StringHandle, f64) as MarkerFieldsTrait>::FIELD_KIND_COUNTS.number_field_count,
            1
        );

        assert_eq!(
            <(StringHandle, f64, StringHandle, f64) as MarkerFieldsTrait>::FIELD_KIND_COUNTS
                .string_field_count,
            2
        );
        assert_eq!(
            <(StringHandle, f64, StringHandle, f64) as MarkerFieldsTrait>::FIELD_KIND_COUNTS
                .number_field_count,
            2
        );
    }
}
