//! Async helpers that orchestrate loading a symbol map or binary from a
//! [`LibraryInfo`] by iterating over the candidate paths returned by the
//! `wholesym::FileResolver` and driving a per-candidate `LoadSymbolMap` /
//! `LoadBinary` state machine to completion.
//!
//! These functions previously lived inside `samply-symbols` as the
//! `LoadSymbolMapForLibraryInfo` / `LoadBinaryForLibraryInfo` /
//! `Load{SymbolMap,Binary}ForDyldCacheImage` state machines. They moved here
//! because the candidate enumeration depends on `wholesym::FileResolver`'s inherent
//! knowledge of where files might live; keeping it as plain `async fn`s in
//! `wholesym` makes the iteration shorter and lets `samply-symbols` focus on
//! single-candidate loads.

#[cfg(feature = "api")]
use samply_symbols::{BinaryImage, LoadBinary};
use samply_symbols::{Error, LibraryInfo, LoadSymbolMap, MultiArchDisambiguator, SymbolMap};

use crate::driver::drive_symbol_map;
#[cfg(feature = "api")]
use crate::driver::drive_with_resolver;
#[cfg(feature = "api")]
use crate::helper::WholesymFileContents;
use crate::helper::{CandidatePathInfo, FileResolver, WholesymFileTypes};

/// Try each debug-file candidate from `helper.get_candidate_paths_for_debug_file`,
/// returning the first symbol map whose `debug_id` matches `info.debug_id`.
///
/// If `helper.get_symbol_map_for_library` already has a precomputed symbol
/// map, that is returned immediately without consulting any candidate paths.
pub async fn load_symbol_map_for_library_info(
    helper: &FileResolver,
    info: &LibraryInfo,
) -> Result<SymbolMap<WholesymFileTypes>, Error> {
    if let Some((fl, sm)) = helper.get_symbol_map_for_library(info) {
        return Ok(SymbolMap::with_symbol_map_trait(fl, sm));
    }

    let debug_id = info
        .debug_id
        .ok_or(Error::NotEnoughInformationToIdentifySymbolMap)?;
    let candidates = helper
        .get_candidate_paths_for_debug_file(info)
        .map_err(|e| {
            Error::HelperErrorDuringGetCandidatePathsForDebugFile(Box::new(info.clone()), e)
        })?;

    let mut errors: Vec<Error> = Vec::new();
    for candidate in candidates {
        let result = drive_single_candidate_symbol_map(helper, candidate, debug_id).await;
        match result {
            Ok(sm) if sm.debug_id() == debug_id => return Ok(sm),
            Ok(sm) => errors.push(Error::UnmatchedDebugId(sm.debug_id(), debug_id)),
            Err(e) => errors.push(e),
        }
    }
    Err(match errors.len() {
        0 => Error::NoCandidatePathForDebugFile(Box::new(info.clone())),
        1 => errors.pop().unwrap(),
        _ => Error::NoSuccessfulCandidate(errors),
    })
}

async fn drive_single_candidate_symbol_map(
    helper: &FileResolver,
    candidate: CandidatePathInfo,
    debug_id: debugid::DebugId,
) -> Result<SymbolMap<WholesymFileTypes>, Error> {
    let mut sm = match candidate {
        CandidatePathInfo::SingleFile(fl) => LoadSymbolMap::<WholesymFileTypes>::new(
            fl,
            Some(MultiArchDisambiguator::DebugId(debug_id)),
        ),
        CandidatePathInfo::InDyldCache {
            dyld_cache_path,
            dylib_path,
        } => LoadSymbolMap::<WholesymFileTypes>::for_dyld_cache(dyld_cache_path, dylib_path),
    };
    drive_symbol_map(&mut sm, helper).await;
    sm.finish()
}

/// Try each binary candidate from `helper.get_candidate_paths_for_binary`,
/// returning the first matching binary.
#[cfg(feature = "api")]
pub async fn load_binary_for_library_info(
    helper: &FileResolver,
    info: &LibraryInfo,
) -> Result<BinaryImage<WholesymFileContents>, Error> {
    if info.code_id.is_none() && (info.debug_name.is_none() || info.debug_id.is_none()) {
        return Err(Error::NotEnoughInformationToIdentifyBinary);
    }

    let candidates = helper
        .get_candidate_paths_for_binary(info)
        .map_err(Error::HelperErrorDuringGetCandidatePathsForBinary)?;
    let disambiguator = match (&info.debug_id, &info.arch) {
        (Some(debug_id), _) => Some(MultiArchDisambiguator::DebugId(*debug_id)),
        (None, Some(arch)) => Some(MultiArchDisambiguator::Arch(arch.clone())),
        (None, None) => None,
    };

    let mut last_err: Option<Error> = None;
    for candidate in candidates {
        let result = drive_single_candidate_binary(
            helper,
            candidate,
            info.name.clone(),
            disambiguator.clone(),
        )
        .await;
        match result {
            Ok(image) => match (info.debug_id, info.code_id.as_ref()) {
                (Some(expected_debug_id), _) => {
                    if image.debug_id() == Some(expected_debug_id) {
                        return Ok(image);
                    }
                    last_err = Some(Error::UnmatchedDebugIdOptional(
                        expected_debug_id,
                        image.debug_id(),
                    ));
                }
                (None, Some(expected_code_id)) => {
                    if image.code_id().as_ref() == Some(expected_code_id) {
                        return Ok(image);
                    }
                    last_err = Some(Error::UnmatchedCodeId(
                        expected_code_id.clone(),
                        image.code_id(),
                    ));
                }
                (None, None) => return Ok(image),
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err
        .unwrap_or_else(|| Error::NoCandidatePathForBinary(info.debug_name.clone(), info.debug_id)))
}

#[cfg(feature = "api")]
async fn drive_single_candidate_binary(
    helper: &FileResolver,
    candidate: CandidatePathInfo,
    name: Option<String>,
    disambiguator: Option<MultiArchDisambiguator>,
) -> Result<BinaryImage<WholesymFileContents>, Error> {
    let mut sm = match candidate {
        CandidatePathInfo::SingleFile(fl) => {
            LoadBinary::<WholesymFileTypes>::new(fl, name, None, disambiguator)
        }
        CandidatePathInfo::InDyldCache {
            dyld_cache_path,
            dylib_path,
        } => LoadBinary::<WholesymFileTypes>::for_dyld_cache(dyld_cache_path, dylib_path),
    };
    drive_with_resolver(&mut sm, helper).await;
    sm.finish()
}
