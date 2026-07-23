use std::io::Write;

use crate::timestamp::{write_timestamps_as_deltas, write_timestamps_as_deltas_with_permutation};
use crate::writer::Writer;
use crate::{GraphColor, ProcessHandle, Timestamp};

/// A handle that identifies a counter in a [`Profile`](crate::Profile). Created
/// with [`Profile::add_counter`](crate::Profile::add_counter).
///
/// Counters track a numeric quantity over time (e.g. resident memory, allocated
/// bytes) and are rendered as graphs in the profiler UI. Samples are added with
/// [`Profile::add_counter_sample`](crate::Profile::add_counter_sample).
///
/// The handle is specific to the [`Profile`](crate::Profile) instance it was
/// created from.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CounterHandle(pub(crate) usize);

/// How a counter's samples are graphed in the profiler UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterGraphType {
    /// Values are absolute levels (e.g. current memory usage).
    LineAccumulated,
    /// Values are per-sample deltas that should be displayed as a rate.
    LineRate,
}

impl CounterGraphType {
    fn as_json_str(self) -> &'static str {
        match self {
            CounterGraphType::LineAccumulated => "line-accumulated",
            CounterGraphType::LineRate => "line-rate",
        }
    }
}

/// The per-sample data source a counter tooltip row reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterTooltipDataSource {
    /// `samples.count[i]`.
    Count,
    /// `samples.count[i] / sampleTimeDelta[i]` (per ms).
    Rate,
    /// `rate / maxCounterSampleCountPerMs` (e.g. process CPU).
    CpuRatio,
    /// `accumulatedCounts[i] - minCount` (cumulative sum minus baseline).
    Accumulated,
    /// The count range across the visible (committed) graph.
    CountRange,
    /// `Σ samples.count[i]` over the preview selection.
    SelectionTotal,
    /// `selection-total / selection-duration` (per ms).
    SelectionRate,
    /// `Σ samples.count[i]` over the committed range.
    CommittedRangeTotal,
    /// `samples.number[i]`. The row is omitted when the column is absent.
    SampleNumber,
}

impl CounterTooltipDataSource {
    fn as_json_str(self) -> &'static str {
        match self {
            CounterTooltipDataSource::Count => "count",
            CounterTooltipDataSource::Rate => "rate",
            CounterTooltipDataSource::CpuRatio => "cpu-ratio",
            CounterTooltipDataSource::Accumulated => "accumulated",
            CounterTooltipDataSource::CountRange => "count-range",
            CounterTooltipDataSource::SelectionTotal => "selection-total",
            CounterTooltipDataSource::SelectionRate => "selection-rate",
            CounterTooltipDataSource::CommittedRangeTotal => "committed-range-total",
            CounterTooltipDataSource::SampleNumber => "sample-number",
        }
    }
}

/// The base unit used to format a tooltip row's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterTooltipUnit {
    Bytes,
    BytesPerSecond,
    Percent,
    Number,
}

impl CounterTooltipUnit {
    fn as_json_str(self) -> &'static str {
        match self {
            CounterTooltipUnit::Bytes => "bytes",
            CounterTooltipUnit::BytesPerSecond => "bytes-per-second",
            CounterTooltipUnit::Percent => "percent",
            CounterTooltipUnit::Number => "number",
        }
    }
}

/// Optional CO₂e estimate rendered alongside the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterTooltipCo2 {
    PerByte,
    PerWatthour,
}

impl CounterTooltipCo2 {
    fn as_json_str(self) -> &'static str {
        match self {
            CounterTooltipCo2::PerByte => "per-byte",
            CounterTooltipCo2::PerWatthour => "per-watthour",
        }
    }
}

/// Auto-scaling unit ladder applied to a tooltip row's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterTooltipScale {
    /// kW / W / mW / µW.
    Power,
    /// Energy units (Wh / mWh / µWh / ...).
    Energy,
}

impl CounterTooltipScale {
    fn as_json_str(self) -> &'static str {
        match self {
            CounterTooltipScale::Power => "power",
            CounterTooltipScale::Energy => "energy",
        }
    }
}

/// How a counter tooltip row's value should be formatted.
#[derive(Debug, Clone)]
pub struct CounterTooltipFormat {
    /// The base formatter for the value.
    pub unit: CounterTooltipUnit,
    /// When set, an additional CO₂e estimate is shown next to the value.
    pub co2: Option<CounterTooltipCo2>,
    /// When set, the value is rendered using the named auto-scaling unit
    /// ladder.
    pub scale: Option<CounterTooltipScale>,
}

impl CounterTooltipFormat {
    fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            w.name("unit")?;
            w.string_value(self.unit.as_json_str())?;
            if let Some(co2) = self.co2 {
                w.name("co2")?;
                w.string_value(co2.as_json_str())?;
            }
            if let Some(scale) = self.scale {
                w.name("scale")?;
                w.string_value(scale.as_json_str())?;
            }
            Ok(())
        })
    }
}

/// One row inside a counter's hover tooltip.
#[derive(Debug, Clone)]
pub enum CounterTooltipRow {
    /// A row that reads a per-sample value and formats it.
    Value {
        /// Where the numeric value comes from.
        source: CounterTooltipDataSource,
        /// How the value is formatted for display.
        format: CounterTooltipFormat,
        /// English text used as a fallback when no translation applies.
        label: String,
        /// Optional stable identifier the renderer maps to a translation.
        label_key: Option<String>,
        /// When true, the row is hidden unless there is a non-empty preview
        /// selection.
        requires_preview_selection: bool,
    },
    /// A visual separator between groups of rows.
    Separator,
}

impl CounterTooltipRow {
    fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            match self {
                CounterTooltipRow::Value {
                    source,
                    format,
                    label,
                    label_key,
                    requires_preview_selection,
                } => {
                    w.name("type")?;
                    w.string_value("value")?;
                    w.name("source")?;
                    w.string_value(source.as_json_str())?;
                    w.name("format")?;
                    format.write_json(w)?;
                    w.name("label")?;
                    w.string_value(label)?;
                    if let Some(key) = label_key {
                        w.name("labelKey")?;
                        w.string_value(key)?;
                    }
                    if *requires_preview_selection {
                        w.name("requiresPreviewSelection")?;
                        w.bool_value(true)?;
                    }
                }
                CounterTooltipRow::Separator => {
                    w.name("type")?;
                    w.string_value("separator")?;
                }
            }
            Ok(())
        })
    }
}

/// Metadata describing how a counter should be rendered in the profiler UI.
#[derive(Debug, Clone)]
pub struct CounterDisplayConfig {
    /// The kind of graph used to render the counter's samples.
    pub graph_type: CounterGraphType,
    /// The unit of the counter's values, e.g. `"bytes"`, `"pWh"`, `"percent"`.
    /// Use an empty string if there is no meaningful unit.
    pub unit: String,
    /// The color used to render the graph.
    pub color: GraphColor,
    /// The marker schema display location used to filter markers shown next
    /// to the counter track (e.g. `"timeline-memory"`). `None` if no markers
    /// should be shown.
    pub marker_schema_location: Option<String>,
    /// Controls the default vertical position of this counter's track.
    /// Lower values appear closer to the top.
    pub sort_weight: i32,
    /// The human-readable label shown in the track sidebar.
    pub label: String,
    /// Describes the rows shown in the counter's hover tooltip.
    pub tooltip_rows: Vec<CounterTooltipRow>,
}

impl CounterDisplayConfig {
    pub fn for_memory() -> Self {
        Self {
            graph_type: CounterGraphType::LineAccumulated,
            unit: "bytes".to_owned(),
            color: GraphColor::Orange,
            marker_schema_location: Some("timeline-memory".to_owned()),
            sort_weight: 20,
            label: "Memory".to_owned(),
            tooltip_rows: memory_tooltip_rows(),
        }
    }
    pub fn for_power(label: &str) -> Self {
        Self {
            graph_type: CounterGraphType::LineRate,
            unit: "pWh".to_owned(),
            color: GraphColor::Grey,
            marker_schema_location: None,
            sort_weight: 30,
            label: label.to_owned(),
            tooltip_rows: power_tooltip_rows(),
        }
    }

    pub fn for_bandwidth() -> Self {
        Self {
            graph_type: CounterGraphType::LineRate,
            unit: "bytes".to_owned(),
            color: GraphColor::Blue,
            marker_schema_location: None,
            sort_weight: 10,
            label: "Bandwidth".to_owned(),
            tooltip_rows: bandwidth_tooltip_rows(),
        }
    }

    pub fn for_process_cpu() -> Self {
        Self {
            graph_type: CounterGraphType::LineRate,
            unit: "percent".to_owned(),
            color: GraphColor::Grey,
            marker_schema_location: None,
            sort_weight: 40,
            label: "Process CPU".to_owned(),
            tooltip_rows: process_cpu_tooltip_rows(),
        }
    }

    pub fn default_with_label(name: &str) -> Self {
        Self {
            graph_type: CounterGraphType::LineRate,
            unit: String::new(),
            color: GraphColor::Grey,
            marker_schema_location: None,
            sort_weight: 50,
            label: name.to_owned(),
            tooltip_rows: generic_tooltip_rows(name),
        }
    }

    fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.object(|w| {
            w.name("graphType")?;
            w.string_value(self.graph_type.as_json_str())?;
            w.name("unit")?;
            w.string_value(&self.unit)?;
            w.name("color")?;
            self.color.write_json(w)?;
            w.name("markerSchemaLocation")?;
            match &self.marker_schema_location {
                Some(s) => w.string_value(s)?,
                None => w.null_value()?,
            }
            w.name("sortWeight")?;
            w.number_value(self.sort_weight)?;
            w.name("label")?;
            w.string_value(&self.label)?;
            w.name("tooltipRows")?;
            w.array(|w| {
                for row in &self.tooltip_rows {
                    row.write_json(w)?;
                }
                Ok(())
            })
        })
    }
}

fn value_row(
    source: CounterTooltipDataSource,
    unit: CounterTooltipUnit,
    co2: Option<CounterTooltipCo2>,
    scale: Option<CounterTooltipScale>,
    label: &str,
    label_key: Option<&str>,
    requires_preview_selection: bool,
) -> CounterTooltipRow {
    CounterTooltipRow::Value {
        source,
        format: CounterTooltipFormat { unit, co2, scale },
        label: label.to_owned(),
        label_key: label_key.map(str::to_owned),
        requires_preview_selection,
    }
}

fn bandwidth_tooltip_rows() -> Vec<CounterTooltipRow> {
    use CounterTooltipCo2::PerByte;
    use CounterTooltipDataSource::*;
    use CounterTooltipUnit::*;
    vec![
        value_row(
            Rate,
            BytesPerSecond,
            Some(PerByte),
            None,
            "Transfer speed for this sample",
            Some("bandwidth-speed"),
            false,
        ),
        value_row(
            SampleNumber,
            Number,
            None,
            None,
            "read/write operations since the previous sample",
            Some("bandwidth-operations"),
            false,
        ),
        CounterTooltipRow::Separator,
        value_row(
            Accumulated,
            Bytes,
            Some(PerByte),
            None,
            "Data transferred up to this time",
            Some("bandwidth-cumulative"),
            false,
        ),
        value_row(
            CountRange,
            Bytes,
            Some(PerByte),
            None,
            "Data transferred in the visible range",
            Some("bandwidth-total-graph"),
            false,
        ),
        value_row(
            SelectionTotal,
            Bytes,
            Some(PerByte),
            None,
            "Data transferred in the current selection",
            Some("bandwidth-total-selection"),
            true,
        ),
    ]
}

fn memory_tooltip_rows() -> Vec<CounterTooltipRow> {
    use CounterTooltipDataSource::*;
    use CounterTooltipUnit::*;
    vec![
        value_row(
            Accumulated,
            Bytes,
            None,
            None,
            "relative memory at this time",
            Some("memory-relative"),
            false,
        ),
        value_row(
            CountRange,
            Bytes,
            None,
            None,
            "memory range in graph",
            Some("memory-range"),
            false,
        ),
        value_row(
            SampleNumber,
            Number,
            None,
            None,
            "allocations and deallocations since the previous sample",
            Some("memory-operations"),
            false,
        ),
    ]
}

fn power_tooltip_rows() -> Vec<CounterTooltipRow> {
    use CounterTooltipCo2::PerWatthour;
    use CounterTooltipDataSource::*;
    use CounterTooltipScale::{Energy, Power};
    use CounterTooltipUnit::Number;
    vec![
        value_row(
            Count,
            Number,
            Some(PerWatthour),
            Some(Power),
            "Power",
            Some("power"),
            false,
        ),
        value_row(
            SelectionTotal,
            Number,
            Some(PerWatthour),
            Some(Energy),
            "Energy used in the current selection",
            Some("power-energy-preview"),
            true,
        ),
        value_row(
            SelectionRate,
            Number,
            Some(PerWatthour),
            Some(Power),
            "Average power in the current selection",
            Some("power-average-preview"),
            true,
        ),
        value_row(
            CommittedRangeTotal,
            Number,
            Some(PerWatthour),
            Some(Energy),
            "Energy used in the visible range",
            Some("power-energy-range"),
            false,
        ),
    ]
}

fn process_cpu_tooltip_rows() -> Vec<CounterTooltipRow> {
    vec![value_row(
        CounterTooltipDataSource::CpuRatio,
        CounterTooltipUnit::Percent,
        None,
        None,
        "CPU",
        Some("cpu"),
        false,
    )]
}

fn generic_tooltip_rows(name: &str) -> Vec<CounterTooltipRow> {
    vec![value_row(
        CounterTooltipDataSource::Count,
        CounterTooltipUnit::Number,
        None,
        None,
        name,
        None,
        false,
    )]
}

#[derive(Debug)]
pub struct Counter {
    name: String,
    category: String,
    description: String,
    process: ProcessHandle,
    pid: String,
    samples: CounterSamples,
    display: CounterDisplayConfig,
}

impl Counter {
    pub fn new(
        name: &str,
        category: &str,
        display: CounterDisplayConfig,
        description: &str,
        process: ProcessHandle,
        pid: &str,
    ) -> Self {
        Counter {
            name: name.to_owned(),
            category: category.to_owned(),
            description: description.to_owned(),
            process,
            pid: pid.to_owned(),
            samples: CounterSamples::new(),
            display,
        }
    }

    pub fn process(&self) -> ProcessHandle {
        self.process
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        value_delta: f64,
        number_of_operations_delta: u32,
    ) {
        self.samples
            .add_sample(timestamp, value_delta, number_of_operations_delta)
    }

    pub fn set_color(&mut self, color: GraphColor) {
        self.display.color = color;
    }

    pub fn set_display(&mut self, display: CounterDisplayConfig) {
        self.display = display;
    }

    pub(crate) fn write_json<W: Write>(
        &self,
        w: &mut Writer<W>,
        main_thread_index: usize,
    ) -> std::io::Result<()> {
        w.object(|w| {
            w.name("category")?;
            w.string_value(&self.category)?;
            w.name("name")?;
            w.string_value(&self.name)?;
            w.name("description")?;
            w.string_value(&self.description)?;
            w.name("mainThreadIndex")?;
            w.number_value(main_thread_index)?;
            w.name("pid")?;
            w.string_value(&self.pid)?;
            w.name("samples")?;
            self.samples.write_json(w)?;
            w.name("display")?;
            self.display.write_json(w)
        })
    }
}

#[derive(Debug)]
struct CounterSamples {
    time: Vec<Timestamp>,
    number: Vec<u32>,
    count: Vec<f64>,

    is_sorted_by_time: bool,
    last_sample_timestamp: Timestamp,
}

impl CounterSamples {
    pub fn new() -> Self {
        Self {
            time: Vec::new(),
            number: Vec::new(),
            count: Vec::new(),

            is_sorted_by_time: true,
            last_sample_timestamp: Timestamp::from_nanos_since_reference(0),
        }
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        value_delta: f64,
        number_of_operations_delta: u32,
    ) {
        self.time.push(timestamp);
        self.count.push(value_delta);
        self.number.push(number_of_operations_delta);

        if timestamp < self.last_sample_timestamp {
            self.is_sorted_by_time = false;
        }
        self.last_sample_timestamp = timestamp;
    }

    fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let len = self.time.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;

            if self.is_sorted_by_time {
                w.name("count")?;
                w.array(|w| {
                    for c in &self.count {
                        w.fp(*c)?;
                    }
                    Ok(())
                })?;
                w.name("number")?;
                w.number_array(&self.number)?;
                w.name("timeDeltas")?;
                write_timestamps_as_deltas(w, &self.time)?;
            } else {
                let mut indexes: Vec<usize> = (0..self.time.len()).collect();
                indexes.sort_unstable_by_key(|index| self.time[*index]);
                w.name("count")?;
                w.array(|w| {
                    for &i in &indexes {
                        w.fp(self.count[i])?;
                    }
                    Ok(())
                })?;
                w.name("number")?;
                w.array(|w| {
                    for &i in &indexes {
                        w.number_value(self.number[i])?;
                    }
                    Ok(())
                })?;
                w.name("timeDeltas")?;
                write_timestamps_as_deltas_with_permutation(w, &self.time, &indexes)?;
            }

            Ok(())
        })
    }
}
