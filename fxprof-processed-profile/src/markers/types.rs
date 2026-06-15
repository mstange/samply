use bitflags::bitflags;
use serde_derive::Serialize;

use crate::Timestamp;

/// The handle for a marker. Returned from [`Profile::add_marker`](crate::Profile::add_marker).
///
/// Keep the handle if you want to attach a stack to the marker afterwards via
/// [`Profile::set_marker_stack`](crate::Profile::set_marker_stack); otherwise it
/// can be discarded.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct MarkerHandle(pub(crate) usize);

/// The handle for a marker type. Returned from
/// [`Profile::register_marker_type`](crate::Profile::register_marker_type) (for
/// runtime-defined schemas) or
/// [`Profile::static_schema_marker_type`](crate::Profile::static_schema_marker_type)
/// (for compile-time [`Marker`](crate::Marker) types).
///
/// The handle identifies the schema all of a type's markers share, and is what
/// implementations of [`DynamicSchemaMarker::marker_type`](crate::DynamicSchemaMarker::marker_type)
/// must return.
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

/// The kind of a marker field.
///
/// Each kind has its own format enum which provides the concrete display format:
/// see [`MarkerStringFieldFormat`](crate::MarkerStringFieldFormat),
/// [`MarkerNumberFieldFormat`](crate::MarkerNumberFieldFormat), and
/// [`MarkerFlowFieldFormat`](crate::MarkerFlowFieldFormat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MarkerFieldKind {
    /// A string-valued field. The concrete format is one of
    /// [`MarkerStringFieldFormat`](crate::MarkerStringFieldFormat).
    String,
    /// A numeric (`f64`) field. The concrete format is one of
    /// [`MarkerNumberFieldFormat`](crate::MarkerNumberFieldFormat).
    Number,
    /// A flow field — a `u64` identifier that links related markers together.
    /// The concrete format is one of [`MarkerFlowFieldFormat`](crate::MarkerFlowFieldFormat).
    Flow,
}

bitflags! {
    /// Locations in the profiler UI where markers can be displayed.
    ///
    /// Combine flags with the bitwise `|` operator:
    ///
    /// ```
    /// use fxprof_processed_profile::MarkerLocations;
    /// let locations = MarkerLocations::MARKER_CHART | MarkerLocations::MARKER_TABLE;
    /// ```
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
///
/// These are named colors from the Firefox Profiler's graph palette (not
/// arbitrary RGB values). When a marker schema uses one of these, the front-end
/// picks the matching palette color.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphColor {
    /// Blue.
    Blue,
    /// Green.
    Green,
    /// Grey.
    Grey,
    /// Ink (dark blue / near-black).
    Ink,
    /// Magenta.
    Magenta,
    /// Orange.
    Orange,
    /// Purple.
    Purple,
    /// Red.
    Red,
    /// Teal.
    Teal,
    /// Yellow.
    Yellow,
}
