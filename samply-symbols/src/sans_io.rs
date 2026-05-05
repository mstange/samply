//! Sans-IO state machines.
//!
//! These types expose the "I need this file" requests of the symbol-loading
//! and lookup graphs as plain values. A driver fetches the requested files
//! (sync, async, RPC, threadpool — its choice) and feeds the bytes back in
//! via `provide`. The state machines themselves never perform I/O.
//!
//! See `sans-io-plan.md` in the workspace root for the design rationale.

use samply_debugid::ElfBuildId;

use crate::shared::{FileLoadError, FileTypes};

pub(crate) mod binary;
pub(crate) mod dyld_cache_load;
pub(crate) mod elf_load;
mod lookup;
mod source_file;
pub(crate) mod symbol_map;

pub use binary::LoadBinary;
pub use dyld_cache_load::DyldCacheLoad;
pub use elf_load::ElfLoad;
pub use lookup::{LookupOutput, LookupQuery};
pub use source_file::{LoadExternalFile, LoadSourceFile};
pub use symbol_map::LoadSymbolMap;

/// A sans-IO state machine that asks for files (`H::FL` → `H::F`) one at a
/// time and consumes the resulting bytes to make progress.
///
/// Implemented by `LoadBinary`, `LoadSourceFile`, `LoadExternalFile`,
/// `LookupQuery`, and `DyldCacheLoad`. The trait is what lets a driver in a
/// downstream crate (e.g. `wholesym`) be written once and reused across all
/// of them.
///
/// `LoadSymbolMap` does not implement this trait; it additionally surfaces
/// candidate-resolution requests via [`SymbolMapLoadStep`] and must be driven
/// directly.
pub trait NeedsFiles<H: FileTypes> {
    /// Inspect what the state machine needs next. `LoadStep::NeedFile`
    /// indicates a file the driver should fetch and feed back via `provide`.
    fn poll(&self) -> LoadStep<'_, H::FL>;
    /// Provide the bytes (or error) requested by the previous `poll`.
    fn provide(&mut self, result: Result<H::F, FileLoadError>);
}

/// What a basic file-loading state machine asks a driver for next.
///
/// Returned by state machines that only need to fetch files: `LoadBinary`,
/// `LoadSourceFile`, `LoadExternalFile`, `LookupQuery`, and `DyldCacheLoad`.
/// `LoadSymbolMap` (and the embedded `ElfLoad`) need additional candidate
/// resolution and use [`SymbolMapLoadStep`] instead.
pub enum LoadStep<'a, FL> {
    /// Fetch the file at `location` and call `provide` with the result.
    ///
    /// If `required` is `false`, an `Err(_)` from the fetch is non-fatal —
    /// the state machine will treat the file as absent and continue.
    NeedFile { location: &'a FL, required: bool },
    /// The state machine is done. Call `finish` to consume it.
    Done,
}

/// What a `LoadSymbolMap` (or `ElfLoad`) state machine asks a driver for next.
///
/// In addition to fetching files, these state machines may need the driver to
/// enumerate `.gnu_debuglink` / `.gnu_debugaltlink` candidate paths so that
/// the parsing logic can try them in order.
pub enum SymbolMapLoadStep<'a, FL> {
    /// Fetch the file at `location` and call `provide` with the result.
    ///
    /// If `required` is `false`, an `Err(_)` from the fetch is non-fatal —
    /// the state machine will treat the file as absent and continue.
    NeedFile { location: &'a FL, required: bool },
    /// Resolve `.gnu_debuglink` candidate paths for the primary file at
    /// `original_location`. The driver should return the candidate locations
    /// (typically by calling `get_candidate_paths_for_gnu_debug_link_dest`
    /// on its helper) via `provide_candidates`. An empty `Vec` is acceptable —
    /// it just means "no debuglink candidates".
    NeedDebugLinkCandidates {
        original_location: &'a FL,
        debug_link_name: &'a str,
    },
    /// Resolve `.gnu_debugaltlink` (supplementary debug file) candidate paths
    /// for the primary file at `original_location`. The driver responds via
    /// `provide_candidates`.
    NeedSupplementaryCandidates {
        original_location: &'a FL,
        sup_path: &'a str,
        sup_build_id: &'a ElfBuildId,
    },
    /// The state machine is done. Call `finish` to consume it.
    Done,
}
