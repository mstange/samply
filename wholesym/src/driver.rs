//! Async drivers for the sans-IO state machines exported by `samply-symbols`
//! and `samply-api`.
//!
//! `samply-symbols` exposes parsing logic as state machines that surface
//! "I need this file" requests as plain values. `wholesym`'s helper knows how
//! to actually fetch those files (local cache, debuginfod, Microsoft symbol
//! servers, breakpad servers, …). This module is the glue between the two:
//! it walks a state machine's `poll → provide` loop, fetching whatever the
//! state machine asks for via the helper.

use samply_symbols::{LoadStep, LoadSymbolMap, NeedsFiles, SymbolMapLoadStep};

use crate::helper::{FileResolver, LocalFileFetcher, WholesymFileTypes};

/// Drive a basic file-loading state machine to completion using a
/// [`FileResolver`].
///
/// Returns once the state machine reaches `LoadStep::Done`. The caller is
/// then expected to call the state machine's `finish` method.
pub async fn drive_with_resolver<S>(sm: &mut S, resolver: &FileResolver)
where
    S: NeedsFiles<WholesymFileTypes>,
{
    loop {
        match sm.poll() {
            LoadStep::NeedFile { location, .. } => {
                let location = location.clone();
                let result = resolver.load_file_impl(location).await;
                sm.provide(result);
            }
            LoadStep::Done => return,
        }
    }
}

/// Drive a basic file-loading state machine to completion using a
/// [`LocalFileFetcher`].
pub async fn drive_with_local<S>(sm: &mut S, fetcher: &LocalFileFetcher)
where
    S: NeedsFiles<WholesymFileTypes>,
{
    loop {
        match sm.poll() {
            LoadStep::NeedFile { location, .. } => {
                let location = location.clone();
                let result = fetcher.load_file_impl(location).await;
                sm.provide(result);
            }
            LoadStep::Done => return,
        }
    }
}

/// Drive a [`LoadSymbolMap`] to completion using a [`FileResolver`].
///
/// `LoadSymbolMap` is the only state machine that surfaces the
/// `NeedDebugLinkCandidates` / `NeedSupplementaryCandidates` variants, so it
/// gets its own driver instead of going through [`NeedsFiles`].
pub async fn drive_symbol_map(sm: &mut LoadSymbolMap<WholesymFileTypes>, resolver: &FileResolver) {
    loop {
        match sm.poll() {
            SymbolMapLoadStep::NeedFile { location, .. } => {
                let location = location.clone();
                let result = resolver.load_file_impl(location).await;
                sm.provide(result);
            }
            SymbolMapLoadStep::NeedDebugLinkCandidates {
                original_location,
                debug_link_name,
            } => {
                let candidates = resolver.get_candidate_paths_for_gnu_debug_link_dest(
                    original_location,
                    debug_link_name,
                );
                sm.provide_candidates(candidates);
            }
            SymbolMapLoadStep::NeedSupplementaryCandidates {
                original_location,
                sup_path,
                sup_build_id,
            } => {
                let candidates = resolver.get_candidate_paths_for_supplementary_debug_file(
                    original_location,
                    sup_path,
                    sup_build_id,
                );
                sm.provide_candidates(candidates);
            }
            SymbolMapLoadStep::Done => return,
        }
    }
}

#[cfg(feature = "api")]
pub use api::drive_api_query;

#[cfg(feature = "api")]
mod api {
    use samply_api::{ApiQueryState, ApiStep, QueryApiJsonResult};
    use samply_symbols::{LoadSourceFile, SourceFilePath};

    use super::drive_with_resolver;
    use crate::helper::{FileResolver, WholesymFileTypes};
    use crate::load_helpers::{load_binary_for_library_info, load_symbol_map_for_library_info};

    /// Drive a [`samply_api::ApiQueryState`] to completion by satisfying its
    /// `NeedSymbolMap` / `NeedBinary` / `NeedSourceFile` / `NeedFile`
    /// requests with the given helper.
    pub async fn drive_api_query(
        mut state: Box<dyn ApiQueryState<WholesymFileTypes> + Send>,
        helper: &FileResolver,
    ) -> QueryApiJsonResult<WholesymFileTypes> {
        loop {
            match state.poll() {
                ApiStep::Done => break,
                ApiStep::NeedSymbolMap(info) => {
                    let result = load_symbol_map_for_library_info(helper, &info).await;
                    state.provide_symbol_map(result);
                }
                ApiStep::NeedBinary(info) => {
                    let result = load_binary_for_library_info(helper, &info).await;
                    state.provide_binary(result);
                }
                ApiStep::NeedSourceFile {
                    debug_file,
                    source_file_path,
                } => {
                    let path = SourceFilePath::RawPath(source_file_path.into());
                    match LoadSourceFile::<WholesymFileTypes>::new(&debug_file, &path) {
                        Ok(mut sm) => {
                            drive_with_resolver(&mut sm, helper).await;
                            state.provide_source_file(sm.finish());
                        }
                        Err(e) => state.provide_source_file(Err(e)),
                    }
                }
                ApiStep::NeedFile { location, .. } => {
                    let result = helper.load_file_impl(location).await;
                    state.provide_file(result);
                }
            }
        }
        state.finish()
    }
}
