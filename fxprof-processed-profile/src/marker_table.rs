use std::io::Write;

use crate::category::CategoryHandle;
use crate::markers::{InternalMarkerSchema, MarkerFieldValueConsumer};
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::timestamp::write_optional_timestamp_column_as_zero_default;
use crate::writer::Writer;
use crate::{
    DynamicSchemaMarker, DynamicSchemaMarkerFieldFormat, MarkerHandle, MarkerStringFieldFormat,
    MarkerTiming, MarkerTypeHandle, StackHandle, Timestamp,
};

#[derive(Debug, Clone, Default)]
pub struct MarkerTable {
    marker_categories: Vec<CategoryHandle>,
    marker_name_string_indexes: Vec<StringHandle>,
    marker_starts: Vec<Option<Timestamp>>,
    marker_ends: Vec<Option<Timestamp>>,
    marker_phases: Vec<Phase>,
    marker_type_handles: Vec<MarkerTypeHandle>,
    marker_stacks: Vec<Option<StackHandle>>,
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

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<StackHandle>) {
        self.marker_stacks[marker.0] = stack_index;
    }

    pub fn with_remapped_stacks(mut self, old_stack_to_new_stack: &[Option<StackHandle>]) -> Self {
        self.marker_stacks = self
            .marker_stacks
            .into_iter()
            .map(|stack| match stack {
                Some(s) => old_stack_to_new_stack[s.0 as usize],
                None => None,
            })
            .collect();
        self
    }

    pub(crate) fn write_json<W: Write>(
        &self,
        w: &mut Writer<W>,
        schemas: &[InternalMarkerSchema],
        string_table: &ProfileStringTable,
    ) -> std::io::Result<()> {
        let len = self.marker_name_string_indexes.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("category")?;
            w.array(|w| {
                for c in &self.marker_categories {
                    c.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("data")?;
            self.write_data_column(w, schemas, string_table)?;
            w.name("endTime")?;
            write_optional_timestamp_column_as_zero_default(w, &self.marker_ends)?;
            w.name("name")?;
            w.array(|w| {
                for n in &self.marker_name_string_indexes {
                    n.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("phase")?;
            w.array(|w| {
                for p in &self.marker_phases {
                    w.number_value(*p as u8)?;
                }
                Ok(())
            })?;
            w.name("startTime")?;
            write_optional_timestamp_column_as_zero_default(w, &self.marker_starts)?;
            Ok(())
        })
    }

    fn write_data_column<W: Write>(
        &self,
        w: &mut Writer<W>,
        schemas: &[InternalMarkerSchema],
        string_table: &ProfileStringTable,
    ) -> std::io::Result<()> {
        let len = self.marker_name_string_indexes.len();
        let mut remaining_string_fields = &self.marker_field_string_values[..];
        let mut remaining_number_fields = &self.marker_field_number_values[..];
        let mut remaining_flow_fields = &self.marker_field_flow_values[..];
        w.array(|w| {
            for i in 0..len {
                let marker_type_handle = self.marker_type_handles[i];
                let stack_index = self.marker_stacks[i];
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
                write_marker_data_element(
                    w,
                    string_table,
                    stack_index,
                    schema,
                    string_fields,
                    number_fields,
                    flow_fields,
                )?;
            }
            Ok(())
        })
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
        let hex_string = format!("{flow:016x}");
        let flow_string_handle = self.string_table.index_for_string(&hex_string);
        self.marker_field_flow_values.push(flow_string_handle);
    }
}

fn write_marker_data_element<W: Write>(
    w: &mut Writer<W>,
    string_table: &ProfileStringTable,
    stack_index: Option<StackHandle>,
    schema: &InternalMarkerSchema,
    mut string_fields: &[StringHandle],
    mut number_fields: &[f64],
    mut flow_fields: &[StringHandle],
) -> std::io::Result<()> {
    w.object(|w| {
        w.name("type")?;
        w.string_value(schema.type_name())?;
        if let Some(stack_index) = stack_index {
            w.name("cause")?;
            w.object(|w| {
                w.name("stack")?;
                w.number_value(stack_index.0)
            })?;
        }
        for field in schema.fields() {
            match &field.format {
                DynamicSchemaMarkerFieldFormat::String(format) => {
                    let value;
                    (value, string_fields) = string_fields.split_first().unwrap();
                    w.name(&field.key)?;
                    if *format == MarkerStringFieldFormat::String {
                        value.write_json(w)?;
                    } else {
                        let str_val = string_table.get_string(*value);
                        w.string_value(str_val)?;
                    }
                }
                DynamicSchemaMarkerFieldFormat::Number(_) => {
                    let value;
                    (value, number_fields) = number_fields.split_first().unwrap();
                    w.name(&field.key)?;
                    w.fp(*value)?;
                }
                DynamicSchemaMarkerFieldFormat::Flow(_) => {
                    let value;
                    (value, flow_fields) = flow_fields.split_first().unwrap();
                    w.name(&field.key)?;
                    value.write_json(w)?;
                }
            }
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Phase {
    Instant = 0,
    Interval = 1,
    IntervalStart = 2,
    IntervalEnd = 3,
}
