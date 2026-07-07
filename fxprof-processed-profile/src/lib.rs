//! This crate allows you to create a profile that can be loaded into
//! the [Firefox Profiler](https://profiler.firefox.com/).
//!
//! Specifically, this uses the ["Processed profile format"](https://github.com/firefox-devtools/profiler/blob/main/docs-developer/processed-profile-format.md).
//!
//! Use [`Profile::new`] to create a new [`Profile`] object. Then add all the
//! information into it. To convert it to JSON, call [`Profile::to_writer`] to
//! stream it into a writer, or [`Profile::to_vec`] to obtain a byte vector.
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
//! let root_frame = profile.handle_for_frame_with_label(root_node_string, CategoryHandle::OTHER, FrameFlags::empty());
//! let first_callee_string = profile.handle_for_string("First callee");
//! let first_callee_frame = profile.handle_for_frame_with_label(first_callee_string, CategoryHandle::OTHER, FrameFlags::empty());
//!
//! let root_stack_node = profile.handle_for_stack(root_frame, None);
//! let first_callee_node = profile.handle_for_stack(first_callee_frame, Some(root_stack_node));
//! profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), Some(first_callee_node), CpuDelta::ZERO, 1);
//!
//! let writer = std::io::BufWriter::new(output_file);
//! profile.to_writer(writer)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Symbolication
//!
//! Profiles built with this crate typically contain raw code addresses rather
//! than function names. The Firefox Profiler can symbolicate addresses on
//! demand, but you can also bake symbol information into the profile ahead of
//! time. Build a [`symbol_info::ProfileSymbolInfo`] containing the addresses
//! you care about and their resolved names / file / line info, then call
//! [`Profile::make_symbolicated_profile`] to produce a new, symbolicated
//! profile.

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
mod profile_shared_data;
mod profile_symbol_info;
mod reference_timestamp;
mod resource_table;
mod sample_table;
mod serialization_helpers;
mod source_table;
mod stack_table;
mod string_table;
mod symbolication;
mod thread;
mod timestamp;

pub use category::{
    Category, CategoryHandle, IntoSubcategoryHandle, Subcategory, SubcategoryHandle,
};
pub use category_color::CategoryColor;
pub use counters::{CounterDisplayConfig, CounterGraphType, CounterHandle};
pub use cpu_delta::CpuDelta;
pub use frame::{FrameAddress, FrameFlags};
pub use global_lib_table::LibraryHandle;
pub use lib_mappings::LibMappings;
pub use library_info::{LibraryInfo, Symbol, SymbolTable};
pub use markers::{
    DynamicSchemaMarker, DynamicSchemaMarkerField, DynamicSchemaMarkerFieldFormat,
    DynamicSchemaMarkerGraph, DynamicSchemaMarkerSchema, FlowId, GraphColor, Marker, MarkerField,
    MarkerFieldKind, MarkerFlowFieldFormat, MarkerGraph, MarkerGraphType, MarkerHandle,
    MarkerLocations, MarkerNumberFieldFormat, MarkerStringFieldFormat, MarkerTiming,
    MarkerTypeHandle, Schema,
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

/// Types describing symbol information for after-the-fact symbolication.
///
/// Used as input to [`Profile::make_symbolicated_profile`]. The flow is:
///
/// 1. Build an unsymbolicated [`Profile`] in the usual way, using raw code
///    addresses for native frames.
/// 2. Collect the addresses you want symbolicated (see
///    [`Profile::native_frame_addresses_per_library`]) and resolve them via
///    whichever symbolicator you use (DWARF, PDB, symbol server, ...).
/// 3. Pack the results into a [`symbol_info::ProfileSymbolInfo`]: one
///    [`symbol_info::LibSymbolInfo`] per [`LibraryHandle`], each containing the
///    resolved [`symbol_info::AddressInfo`] for its addresses. Strings (function
///    names, file paths) are stored once in the [`symbol_info::SymbolStringTable`]
///    and referenced by [`symbol_info::SymbolStringIndex`].
/// 4. Call [`Profile::make_symbolicated_profile`] to obtain a new, symbolicated
///    profile.
pub mod symbol_info {
    pub use crate::profile_symbol_info::*;
}
