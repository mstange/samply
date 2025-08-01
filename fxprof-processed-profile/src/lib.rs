//! This crate allows you to create a profile that can be loaded into
//! the [Firefox Profiler](https://profiler.firefox.com/).
//!
//! Specifically, this uses the ["Processed profile format"](https://github.com/firefox-devtools/profiler/blob/main/docs-developer/processed-profile-format.md).
//!
//! Use [`Profile::new`] to create a new [`Profile`] object. Then add all the
//! information into it. To convert it to JSON, use [`serde_json`], for
//! example [`serde_json::to_writer`] or [`serde_json::to_string`].
//!
//! ## Example
//!
//! ```
//! use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, FrameHandle, FrameFlags, SamplingInterval, Timestamp};
//! use std::time::SystemTime;
//!
//! // Creates the following call tree:
//! //
//! // App process (pid: 54132) > Main thread (tid: 54132000)
//! //
//! // 1  0  Root node
//! // 1  1  - First callee
//!
//! # fn write_profile(output_file: std::fs::File) -> Result<(), Box<dyn std::error::Error>> {
//! let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
//! let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
//! let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
//! profile.set_thread_name(thread, "Main thread");
//!
//! let root_node_string = profile.handle_for_string("Root node");
//! let root_frame = profile.handle_for_frame_with_label(thread, root_node_string, CategoryHandle::OTHER, FrameFlags::empty());
//! let first_callee_string = profile.handle_for_string("First callee");
//! let first_callee_frame = profile.handle_for_frame_with_label(thread, first_callee_string, CategoryHandle::OTHER, FrameFlags::empty());
//!
//! let root_stack_node = profile.handle_for_stack(thread, root_frame, None);
//! let first_callee_node = profile.handle_for_stack(thread, first_callee_frame, Some(root_stack_node));
//! profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), Some(first_callee_node), CpuDelta::ZERO, 1);
//!
//! let writer = std::io::BufWriter::new(output_file);
//! serde_json::to_writer(writer, &profile)?;
//! # Ok(())
//! # }
//! ```

pub use debugid;

mod category;
mod category_color;
mod counters;
mod cpu_delta;
mod fast_hash_map;
mod frame;
mod frame_table;
mod func_table;
mod global_lib_table;
mod lib_mappings;
mod library_info;
mod marker_table;
mod markers;
mod native_symbols;
mod process;
mod profile;
mod profile_symbol_info;
mod reference_timestamp;
mod resource_table;
mod sample_table;
mod serialization_helpers;
mod stack_table;
mod string_table;
mod symbolication;
mod thread;
mod timestamp;

pub use category::{
    Category, CategoryHandle, IntoSubcategoryHandle, Subcategory, SubcategoryHandle,
};
pub use category_color::CategoryColor;
pub use counters::CounterHandle;
pub use cpu_delta::CpuDelta;
pub use frame::{FrameAddress, FrameFlags};
pub use global_lib_table::LibraryHandle;
pub use lib_mappings::LibMappings;
pub use library_info::{LibraryInfo, Symbol, SymbolTable};
pub use markers::{
    GraphColor, Marker, MarkerFieldFormat, MarkerFieldFormatKind, MarkerGraphType, MarkerHandle,
    MarkerLocations, MarkerTiming, MarkerTypeHandle, RuntimeSchemaMarkerField,
    RuntimeSchemaMarkerGraph, RuntimeSchemaMarkerSchema, StaticSchemaMarker,
    StaticSchemaMarkerField, StaticSchemaMarkerGraph,
};
pub use native_symbols::NativeSymbolHandle;
pub use process::ThreadHandle;
pub use profile::{
    FrameHandle, FrameSymbolInfo, Profile, SamplingInterval, SourceLocation, StackHandle,
    TimelineUnit,
};
pub use reference_timestamp::{PlatformSpecificReferenceTimestamp, ReferenceTimestamp};
pub use sample_table::WeightType;
pub use string_table::StringHandle;
pub use thread::ProcessHandle;
pub use timestamp::Timestamp;

/// A module for types used in [`Profile::make_symbolicated_profile`].
pub mod symbol_info {
    pub use crate::profile_symbol_info::*;
}
