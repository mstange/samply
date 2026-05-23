//! Generic sans-IO types shared by the file-loading state machines.
//!
//! `samply_symbols`'s state machines expose their "I need this file" requests
//! as plain values. A driver fetches the requested files (sync, async, RPC,
//! threadpool — its choice) and feeds the bytes back in via `provide`. The
//! state machines themselves never perform I/O.
//!
//! The concrete state machines live next to the data they produce:
//! [`LoadBinary`](crate::LoadBinary) in `binary_image`,
//! [`ElfLoad`](crate::ElfLoad) in `elf`,
//! [`DyldCacheLoad`](crate::macho::DyldCacheLoad) in `macho`,
//! [`LoadSymbolMap`](crate::LoadSymbolMap) and
//! [`LookupQuery`](crate::LookupQuery) in `symbol_map`,
//! [`LoadSourceFile`](crate::LoadSourceFile) in `source_file_path`,
//! [`LoadExternalFile`](crate::LoadExternalFile) in `external_file`.
//!
//! See `sans-io-plan.md` in the workspace root for the design rationale.

use samply_debugid::ElfBuildId;

use crate::shared::{FileLoadError, FileTypes};

/// A sans-IO state machine that asks for files (`FT::FL` → `FT::F`) one at a
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
pub trait NeedsFiles<FT: FileTypes> {
    /// Inspect what the state machine needs next. `LoadStep::NeedFile`
    /// indicates a file the driver should fetch and feed back via `provide`.
    fn poll(&self) -> LoadStep<'_, FT::FL>;
    /// Provide the bytes (or error) requested by the previous `poll`.
    fn provide(&mut self, result: Result<FT::F, FileLoadError>);
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
