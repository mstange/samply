use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::markers::{InternalMarkerSchema, MarkerFieldFormatKind};
use crate::serialization_helpers::SerializableOptionalTimestampColumn;
use crate::string_table::{GlobalStringIndex, GlobalStringTable, StringIndex};
use crate::thread_string_table::{ThreadInternalStringIndex, ThreadStringTable};
use crate::{
    CategoryHandle, Marker, MarkerFieldFormat, MarkerHandle, MarkerTiming, MarkerTypeHandle,
    Timestamp,
};

#[derive(Debug, Clone, Default)]
pub struct MarkerTable {
    marker_categories: Vec<CategoryHandle>,
    marker_name_string_indexes: Vec<ThreadInternalStringIndex>,
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
    /// The [`StringIndex`] can be either a [`ThreadInternalStringIndex`] or a [`GlobalStringIndex`],
    /// depending on the string field's format - if the field format is [`MarkerFieldFormat::String`],
    /// the string index will be thread-internal, for all the other string format variants the
    /// index will be global.
    ///
    /// We make this distinction because, in the actual JSON, we currently only use string indexes for
    /// the [`MarkerFieldFormat::String`] format (serialized as "unique-string"). The other
    /// string format variants currently still use actual strings in the JSON, not string indexes.
    /// So for these we don't want to add the strings to the thread's string table.
    ///
    /// https://github.com/firefox-devtools/profiler/issues/5022 tracks supporting string indexes
    /// for the other string format variants.
    marker_field_string_values: Vec<StringIndex>,
}

impl MarkerTable {
    pub fn new() -> Self {
        Default::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_marker<T: Marker>(
        &mut self,
        name_string_index: ThreadInternalStringIndex,
        marker_type_handle: MarkerTypeHandle,
        schema: &InternalMarkerSchema,
        marker: T,
        timing: MarkerTiming,
        category: CategoryHandle,
        thread_string_table: &mut ThreadStringTable,
        global_string_table: &mut GlobalStringTable,
    ) -> MarkerHandle {
        let (s, e, phase) = match timing {
            MarkerTiming::Instant(s) => (Some(s), None, Phase::Instant),
            MarkerTiming::Interval(s, e) => (Some(s), Some(e), Phase::Interval),
            MarkerTiming::IntervalStart(s) => (Some(s), None, Phase::IntervalStart),
            MarkerTiming::IntervalEnd(e) => (None, Some(e), Phase::IntervalEnd),
        };
        self.marker_categories.push(category);
        self.marker_name_string_indexes.push(name_string_index);
        self.marker_starts.push(s);
        self.marker_ends.push(e);
        self.marker_phases.push(phase);
        self.marker_type_handles.push(marker_type_handle);
        self.marker_stacks.push(None);
        for (field_index, field) in schema.fields().iter().enumerate() {
            match field.format.kind() {
                MarkerFieldFormatKind::String => {
                    let global_string_index = marker.string_field_value(field_index as u32).0;
                    let string_index = if field.format == MarkerFieldFormat::String {
                        // This one ends up as `format: "unique-string"` with an index into the thread string table
                        let thread_string_index = thread_string_table
                            .index_for_global_string(global_string_index, global_string_table);
                        thread_string_index.0
                    } else {
                        // These end up in the JSON as JSON strings, not as string indexes.
                        global_string_index.0
                    };
                    self.marker_field_string_values.push(string_index);
                }
                MarkerFieldFormatKind::Number => {
                    let number_value = marker.number_field_value(field_index as u32);
                    self.marker_field_number_values.push(number_value)
                }
            }
        }

        MarkerHandle(self.marker_categories.len() - 1)
    }

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<usize>) {
        self.marker_stacks[marker.0] = stack_index;
    }

    pub fn as_serializable<'a>(
        &'a self,
        schemas: &'a [InternalMarkerSchema],
        global_string_table: &'a GlobalStringTable,
    ) -> impl Serialize + 'a {
        SerializableMarkerTable {
            marker_table: self,
            global_string_table,
            schemas,
        }
    }
}

struct SerializableMarkerTable<'a> {
    marker_table: &'a MarkerTable,
    global_string_table: &'a GlobalStringTable,
    schemas: &'a [InternalMarkerSchema],
}

impl<'a> Serialize for SerializableMarkerTable<'a> {
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

impl<'a> Serialize for SerializableMarkerTableDataColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let marker_table = self.0.marker_table;
        let schemas = self.0.schemas;
        let global_string_table = self.0.global_string_table;
        let len = marker_table.marker_name_string_indexes.len();
        let mut seq = serializer.serialize_seq(Some(len))?;
        let mut remaining_string_fields = &marker_table.marker_field_string_values[..];
        let mut remaining_number_fields = &marker_table.marker_field_number_values[..];
        for i in 0..len {
            let marker_type_handle = marker_table.marker_type_handles[i];
            let stack_index = marker_table.marker_stacks[i];
            let schema = &schemas[marker_type_handle.0];
            let string_fields;
            let number_fields;
            (string_fields, remaining_string_fields) =
                remaining_string_fields.split_at(schema.string_field_count());
            (number_fields, remaining_number_fields) =
                remaining_number_fields.split_at(schema.number_field_count());
            seq.serialize_element(&SerializableMarkerDataElement {
                global_string_table,
                stack_index,
                schema,
                string_fields,
                number_fields,
            })?;
        }
        seq.end()
    }
}

struct SerializableMarkerDataElement<'a> {
    global_string_table: &'a GlobalStringTable,
    stack_index: Option<usize>,
    schema: &'a InternalMarkerSchema,
    string_fields: &'a [StringIndex],
    number_fields: &'a [f64],
}

impl<'a> Serialize for SerializableMarkerDataElement<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self {
            global_string_table,
            stack_index,
            schema,
            mut string_fields,
            mut number_fields,
        } = self;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &schema.type_name())?;
        if let Some(stack_index) = stack_index {
            map.serialize_entry("cause", &SerializableMarkerCause(*stack_index))?;
        }
        for field in schema.fields() {
            match field.format.kind() {
                MarkerFieldFormatKind::String => {
                    let value;
                    (value, string_fields) = string_fields.split_first().unwrap();
                    if field.format == MarkerFieldFormat::String {
                        map.serialize_entry(&field.key, value)?;
                    } else {
                        let str_val = global_string_table
                            .get_string(GlobalStringIndex(*value))
                            .unwrap();
                        map.serialize_entry(&field.key, str_val)?;
                    }
                }
                MarkerFieldFormatKind::Number => {
                    let value;
                    (value, number_fields) = number_fields.split_first().unwrap();
                    map.serialize_entry(&field.key, &value)?;
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
