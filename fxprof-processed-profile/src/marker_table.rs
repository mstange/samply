use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::markers::{InternalMarkerSchema, MarkerFieldValueConsumer};
use crate::serialization_helpers::SerializableOptionalTimestampColumn;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::{
    CategoryHandle, MarkerHandle, MarkerStringFieldFormat, MarkerTiming, MarkerTypeHandle,
    DynamicSchemaMarker, DynamicSchemaMarkerFieldFormat, Timestamp,
};

#[derive(Debug, Clone, Default)]
pub struct MarkerTable {
    marker_categories: Vec<CategoryHandle>,
    marker_name_string_indexes: Vec<StringHandle>,
    marker_starts: Vec<Option<Timestamp>>,
    marker_ends: Vec<Option<Timestamp>>,
    marker_phases: Vec<Phase>,
    marker_type_handles: Vec<MarkerTypeHandle>,
    marker_stacks: Vec<Option<usize>>,
    /// The field values for any marker fields of [kind](`MarkerFieldFormat::kind`) [`MarkerFieldFormatKind::Number`].
    ///
    /// This Vec can contain zero or more values per marker, depending on the marker's
    /// type's schema's `number_field_count`. For the marker with index i,
    /// its field values will be at index sum_{k in 0..i}(marker_schema[k].number_field_count).
    marker_field_number_values: Vec<f64>,
    /// The field values for any marker fields of [kind](`MarkerFieldFormat::kind`) [`MarkerFieldFormatKind::String`].
    ///
    /// This Vec can contain zero or more values per marker, depending on the marker's
    /// type's schema's `string_field_count`. For the marker with index i,
    /// its field values will be at index sum_{k in 0..i}(marker_schema[k].string_field_count).
    ///
    /// We make this distinction because, in the actual JSON, we currently only use string indexes for
    /// the [`MarkerFieldFormat::String`] format (serialized as "unique-string"). The other
    /// string format variants currently still use actual strings in the JSON, not string indexes.
    /// So for these we don't want to add the strings to the thread's string table.
    ///
    /// https://github.com/firefox-devtools/profiler/issues/5022 tracks supporting string indexes
    /// for the other string format variants.
    marker_field_string_values: Vec<StringHandle>,
    /// The field values for any marker fields of [kind](`MarkerFieldFormat::kind`) [`MarkerFieldFormatKind::Flow`].
    ///
    /// This Vec can contain zero or more values per marker, depending on the marker's
    /// type's schema's `flow_field_count`. For the marker with index i,
    /// its field values will be at index sum_{k in 0..i}(marker_schema[k].flow_field_count).
    ///
    /// Flow identifiers are serialized as string indexes.
    marker_field_flow_values: Vec<StringHandle>,
}

impl MarkerTable {
    pub fn new() -> Self {
        Default::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_marker<T: DynamicSchemaMarker>(
        &mut self,
        string_table: &mut ProfileStringTable,
        name_string_index: StringHandle,
        marker_type_handle: MarkerTypeHandle,
        schema: &InternalMarkerSchema,
        marker: T,
        timing: MarkerTiming,
    ) -> MarkerHandle {
        let (s, e, phase) = match timing {
            MarkerTiming::Instant(s) => (Some(s), None, Phase::Instant),
            MarkerTiming::Interval(s, e) => (Some(s), Some(e), Phase::Interval),
            MarkerTiming::IntervalStart(s) => (Some(s), None, Phase::IntervalStart),
            MarkerTiming::IntervalEnd(e) => (None, Some(e), Phase::IntervalEnd),
        };
        self.marker_categories.push(schema.category());
        self.marker_name_string_indexes.push(name_string_index);
        self.marker_starts.push(s);
        self.marker_ends.push(e);
        self.marker_phases.push(phase);
        self.marker_type_handles.push(marker_type_handle);
        self.marker_stacks.push(None);

        let MarkerTable {
            marker_field_string_values,
            marker_field_number_values,
            marker_field_flow_values,
            ..
        } = self;

        marker.push_field_values(&mut MarkerTableFieldValueConsumer {
            marker_field_string_values,
            marker_field_number_values,
            marker_field_flow_values,
            string_table,
        });

        MarkerHandle(self.marker_categories.len() - 1)
    }

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<usize>) {
        self.marker_stacks[marker.0] = stack_index;
    }

    pub fn with_remapped_stacks(mut self, old_stack_to_new_stack: &[Option<usize>]) -> Self {
        self.marker_stacks = self
            .marker_stacks
            .into_iter()
            .map(|stack| stack.and_then(|s| old_stack_to_new_stack[s]))
            .collect();
        self
    }

    pub fn as_serializable<'a>(
        &'a self,
        schemas: &'a [InternalMarkerSchema],
        string_table: &'a ProfileStringTable,
    ) -> impl Serialize + 'a {
        SerializableMarkerTable {
            marker_table: self,
            string_table,
            schemas,
        }
    }
}

struct MarkerTableFieldValueConsumer<'a> {
    marker_field_string_values: &'a mut Vec<StringHandle>,
    marker_field_number_values: &'a mut Vec<f64>,
    marker_field_flow_values: &'a mut Vec<StringHandle>,
    string_table: &'a mut ProfileStringTable,
}

impl<'a> MarkerFieldValueConsumer for MarkerTableFieldValueConsumer<'a> {
    fn consume_string_field(&mut self, string_handle: StringHandle) {
        self.marker_field_string_values.push(string_handle);
    }

    fn consume_number_field(&mut self, number: f64) {
        self.marker_field_number_values.push(number);
    }

    fn consume_flow_field(&mut self, flow: u64) {
        // Convert flow ID to hex string and store as StringHandle
        let hex_string = format!("{flow:x}");
        let flow_string_handle = self.string_table.index_for_string(&hex_string);
        self.marker_field_flow_values.push(flow_string_handle);
    }
}

struct SerializableMarkerTable<'a> {
    marker_table: &'a MarkerTable,
    string_table: &'a ProfileStringTable,
    schemas: &'a [InternalMarkerSchema],
}

impl Serialize for SerializableMarkerTable<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self { marker_table, .. } = self;
        let len = marker_table.marker_name_string_indexes.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("category", &marker_table.marker_categories)?;
        map.serialize_entry("data", &SerializableMarkerTableDataColumn(self))?;
        map.serialize_entry(
            "endTime",
            &SerializableOptionalTimestampColumn(&marker_table.marker_ends),
        )?;
        map.serialize_entry("name", &marker_table.marker_name_string_indexes)?;
        map.serialize_entry("phase", &marker_table.marker_phases)?;
        map.serialize_entry(
            "startTime",
            &SerializableOptionalTimestampColumn(&marker_table.marker_starts),
        )?;
        map.end()
    }
}

struct SerializableMarkerTableDataColumn<'a>(&'a SerializableMarkerTable<'a>);

impl Serialize for SerializableMarkerTableDataColumn<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let marker_table = self.0.marker_table;
        let schemas = self.0.schemas;
        let string_table = self.0.string_table;
        let len = marker_table.marker_name_string_indexes.len();
        let mut seq = serializer.serialize_seq(Some(len))?;
        let mut remaining_string_fields = &marker_table.marker_field_string_values[..];
        let mut remaining_number_fields = &marker_table.marker_field_number_values[..];
        let mut remaining_flow_fields = &marker_table.marker_field_flow_values[..];
        for i in 0..len {
            let marker_type_handle = marker_table.marker_type_handles[i];
            let stack_index = marker_table.marker_stacks[i];
            let schema = &schemas[marker_type_handle.0];
            let string_fields;
            let number_fields;
            let flow_fields;
            (string_fields, remaining_string_fields) =
                remaining_string_fields.split_at(schema.string_field_count());
            (number_fields, remaining_number_fields) =
                remaining_number_fields.split_at(schema.number_field_count());
            (flow_fields, remaining_flow_fields) =
                remaining_flow_fields.split_at(schema.flow_field_count());
            seq.serialize_element(&SerializableMarkerDataElement {
                string_table,
                stack_index,
                schema,
                string_fields,
                number_fields,
                flow_fields,
            })?;
        }
        seq.end()
    }
}

struct SerializableMarkerDataElement<'a> {
    string_table: &'a ProfileStringTable,
    stack_index: Option<usize>,
    schema: &'a InternalMarkerSchema,
    string_fields: &'a [StringHandle],
    number_fields: &'a [f64],
    flow_fields: &'a [StringHandle],
}

impl Serialize for SerializableMarkerDataElement<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self {
            string_table,
            stack_index,
            schema,
            mut string_fields,
            mut number_fields,
            mut flow_fields,
        } = self;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &schema.type_name())?;
        if let Some(stack_index) = stack_index {
            map.serialize_entry("cause", &SerializableMarkerCause(*stack_index))?;
        }
        for field in schema.fields() {
            match &field.format {
                DynamicSchemaMarkerFieldFormat::String(format) => {
                    let value;
                    (value, string_fields) = string_fields.split_first().unwrap();
                    if *format == MarkerStringFieldFormat::String {
                        map.serialize_entry(&field.key, value)?;
                    } else {
                        let str_val = string_table.get_string(*value);
                        map.serialize_entry(&field.key, str_val)?;
                    }
                }
                DynamicSchemaMarkerFieldFormat::Number(_) => {
                    let value;
                    (value, number_fields) = number_fields.split_first().unwrap();
                    map.serialize_entry(&field.key, value)?;
                }
                DynamicSchemaMarkerFieldFormat::Flow(_) => {
                    let value;
                    (value, flow_fields) = flow_fields.split_first().unwrap();
                    map.serialize_entry(&field.key, value)?;
                }
            }
        }

        map.end()
    }
}

struct SerializableMarkerCause(usize);
impl Serialize for SerializableMarkerCause {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("stack", &self.0)?;
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
