use crate::{Category, Profile, StringHandle};

use super::dynamic_schema::{
    DynamicSchemaMarker, DynamicSchemaMarkerField, DynamicSchemaMarkerFieldFormat,
    DynamicSchemaMarkerGraph, MarkerFieldValueConsumer,
};
use super::field_format::{
    MarkerFlowFieldFormat, MarkerNumberFieldFormat, MarkerStringFieldFormat,
};
use super::field_kind_counts::MarkerFieldKindCounts;
use super::types::{
    GraphColor, MarkerFieldKind, MarkerGraphType, MarkerLocations, MarkerTypeHandle,
};

/// The trait for markers whose schema is known at compile time. Any type which implements
/// [`Marker`] automatically implements the [`Marker`] trait via a blanket impl.
///
/// Markers have a type, a name, and an arbitrary number of fields.
/// The fields of a marker type are defined by the marker type's schema, see [`DynamicSchemaMarkerSchema`](super::dynamic_schema::DynamicSchemaMarkerSchema).
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
    /// [`DynamicSchemaMarkerSchema::type_name`](super::dynamic_schema::DynamicSchemaMarkerSchema::type_name) of this type's schema.
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

/// A wrapper type used in [`Marker::FIELDS`], usually wraps a tuple.
pub struct Schema<FieldsType: MarkerFieldsTrait>(pub FieldsType::Schema);

/// A graph within a marker graph track, used in [`Marker::GRAPHS`].
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

/// The trait that needs to be implemented by the return type of
/// [`Marker::field_values`].
///
/// This trait is implemented for the field value types [`f64`], [`StringHandle`]
/// and [`FlowId`], and for tuples of these types.
pub trait MarkerFieldsTrait {
    type Schema;

    const FIELD_KIND_COUNTS: MarkerFieldKindCounts;

    fn push_field_values(self, consumer: &mut impl MarkerFieldValueConsumer);

    fn to_runtime_field_schema(schema: &Self::Schema) -> Vec<DynamicSchemaMarkerField>;
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

impl<T: MarkerFieldValueType> From<&MarkerField<T>> for DynamicSchemaMarkerField {
    fn from(schema: &MarkerField<T>) -> Self {
        Self {
            key: schema.key.into(),
            label: schema.label.into(),
            format: schema.format.clone().into(),
        }
    }
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
