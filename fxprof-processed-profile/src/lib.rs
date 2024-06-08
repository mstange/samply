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
//! use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, Frame, FrameInfo, FrameFlags, SamplingInterval, Timestamp};
//! use std::time::SystemTime;
//!
//! # fn write_profile(output_file: std::fs::File) -> Result<(), Box<dyn std::error::Error>> {
//! let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
//! let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
//! let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
//! profile.set_thread_name(thread, "Main thread");
//! let stack = vec![
//!     FrameInfo { frame: Frame::Label(profile.intern_string("Root node")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() },
//!     FrameInfo { frame: Frame::Label(profile.intern_string("First callee")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() }
//! ];
//! profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), stack.into_iter(), CpuDelta::ZERO, 1);
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
mod reference_timestamp;
mod resource_table;
mod sample_table;
mod serialization_helpers;
mod stack_table;
mod string_table;
mod thread;
mod thread_string_table;
mod timestamp;

pub use category::{CategoryHandle, CategoryPairHandle};
pub use category_color::CategoryColor;
pub use counters::CounterHandle;
pub use cpu_delta::CpuDelta;
pub use frame::{Frame, FrameFlags, FrameInfo};
pub use global_lib_table::{LibraryHandle, UsedLibraryAddressesIterator};
pub use lib_mappings::LibMappings;
pub use library_info::{LibraryInfo, Symbol, SymbolTable};
pub use markers::{
    Marker, MarkerFieldFormat, MarkerFieldFormatKind, MarkerFieldSchema, MarkerHandle,
    MarkerLocation, MarkerSchema, MarkerStaticField, MarkerTiming, MarkerTypeHandle,
    StaticSchemaMarker,
};
pub use process::ThreadHandle;
pub use profile::{Profile, SamplingInterval, StringHandle};
pub use reference_timestamp::ReferenceTimestamp;
pub use thread::ProcessHandle;
pub use timestamp::*;
