use std::collections::VecDeque;

use debugid::DebugId;
use object::FileKind;

use crate::elf::{
    analyze_elf_primary, build_elf_symbol_map_for_debuglink_match,
    build_elf_symbol_map_no_supplementary, build_elf_symbol_map_with_supplementary, debuglink_crc,
    supplementary_build_id_matches, ElfDebugAltLinkInfo, ElfDebugLinkInfo, ElfPrimaryInfo,
};
use crate::error::Error;
use crate::sans_io::SymbolMapLoadStep;
use crate::shared::{FileContentsWrapper, FileLoadError, FileLocation, FileTypes};
use crate::symbol_map::SymbolMap;

/// State machine for loading an ELF binary's symbol map.
///
/// Handles the full ELF companion-file graph: `.gnu_debuglink` candidates with
/// CRC validation, optional `.dwp`, optional `.gnu_debugaltlink` (supplementary)
/// candidates, and `.gnu_debugdata` (mini-debug-info) fallback.
///
/// The state machine surfaces two kinds of requests via [`SymbolMapLoadStep`]:
///   - `SymbolMapLoadStep::NeedFile { required: false }` — fetch a candidate
///     file. Each fetch is best-effort; an `Err` is treated as the file being
///     absent.
///   - `SymbolMapLoadStep::NeedDebugLinkCandidates` / `NeedSupplementaryCandidates`
///     — enumerate candidate paths for a debug-link / supplementary debug file.
///     The driver answers via [`ElfLoad::provide_candidates`].
pub struct ElfLoad<H: FileTypes> {
    state: ElfLoadState<H>,
}

enum ElfLoadState<H: FileTypes> {
    /// Surfacing `SymbolMapLoadStep::NeedDebugLinkCandidates`. Awaiting
    /// `provide_candidates` to deliver paths.
    NeedDebugLinkCandidates {
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        file_location: H::FL,
        file_kind: FileKind,
        debuglink: ElfDebugLinkInfo,
        debug_id: DebugId,
    },
    /// Awaiting bytes for a `.gnu_debuglink` candidate so we can CRC-check it.
    AwaitingDebugLinkCandidate {
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        file_location: H::FL,
        file_kind: FileKind,
        debuglink: ElfDebugLinkInfo,
        debug_id: DebugId,
        candidates: VecDeque<H::FL>,
        pending: H::FL,
    },
    /// A debuglink candidate matched. Awaiting its (optional) `.dwp`.
    AwaitingDebugLinkDwp {
        candidate_contents: FileContentsWrapper<H::F>,
        file_location: H::FL,
        file_kind: FileKind,
        debug_id: DebugId,
        pending: H::FL,
    },
    /// No (matching) debuglink. Awaiting the primary's optional `.dwp`.
    AwaitingPrimaryDwp {
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        file_location: H::FL,
        file_kind: FileKind,
        pending: H::FL,
    },
    /// Surfacing `SymbolMapLoadStep::NeedSupplementaryCandidates`. Awaiting
    /// `provide_candidates` to deliver paths.
    NeedSupplementaryCandidates {
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        dwp: Option<FileContentsWrapper<H::F>>,
        file_location: H::FL,
        file_kind: FileKind,
        debugaltlink: ElfDebugAltLinkInfo,
    },
    /// Trying supplementary (debugaltlink) candidates.
    AwaitingSupplementaryCandidate {
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        dwp: Option<FileContentsWrapper<H::F>>,
        file_location: H::FL,
        file_kind: FileKind,
        debugaltlink: ElfDebugAltLinkInfo,
        candidates: VecDeque<H::FL>,
        pending: H::FL,
    },
    Done(Result<SymbolMap<H>, Error>),
    Poisoned,
}

impl<H: FileTypes> ElfLoad<H> {
    /// Construct a state machine for an already-loaded primary ELF binary.
    pub fn new(
        file_location: H::FL,
        file_contents: FileContentsWrapper<H::F>,
        file_kind: FileKind,
    ) -> Self {
        let primary_info = match analyze_elf_primary(&file_contents, file_kind) {
            Ok(info) => info,
            Err(e) => {
                return Self {
                    state: ElfLoadState::Done(Err(e)),
                }
            }
        };
        let mut sm = Self {
            state: ElfLoadState::Poisoned,
        };
        sm.start_debuglink_or_advance(file_location, file_contents, primary_info, file_kind);
        sm
    }

    pub fn poll(&self) -> SymbolMapLoadStep<'_, H::FL> {
        match &self.state {
            ElfLoadState::NeedDebugLinkCandidates {
                file_location,
                debuglink,
                ..
            } => SymbolMapLoadStep::NeedDebugLinkCandidates {
                original_location: file_location,
                debug_link_name: &debuglink.name,
            },
            ElfLoadState::AwaitingDebugLinkCandidate { pending, .. }
            | ElfLoadState::AwaitingDebugLinkDwp { pending, .. }
            | ElfLoadState::AwaitingPrimaryDwp { pending, .. }
            | ElfLoadState::AwaitingSupplementaryCandidate { pending, .. } => {
                SymbolMapLoadStep::NeedFile {
                    location: pending,
                    required: false,
                }
            }
            ElfLoadState::NeedSupplementaryCandidates {
                file_location,
                debugaltlink,
                ..
            } => SymbolMapLoadStep::NeedSupplementaryCandidates {
                original_location: file_location,
                sup_path: &debugaltlink.path,
                sup_build_id: &debugaltlink.build_id,
            },
            ElfLoadState::Done(_) => SymbolMapLoadStep::Done,
            ElfLoadState::Poisoned => unreachable!("invalid ElfLoad state"),
        }
    }

    pub fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, ElfLoadState::Poisoned);
        match state {
            ElfLoadState::AwaitingDebugLinkCandidate {
                primary,
                primary_info,
                file_location,
                file_kind,
                debuglink,
                debug_id,
                mut candidates,
                pending: candidate_path,
            } => {
                let matched_contents = match result {
                    Ok(file) => {
                        let candidate = FileContentsWrapper::new(file);
                        match debuglink_crc(&candidate) {
                            Ok(crc) if crc == debuglink.crc => Some(candidate),
                            _ => None,
                        }
                    }
                    Err(_) => None,
                };
                if let Some(candidate_contents) = matched_contents {
                    self.start_debuglink_dwp_with_path(
                        candidate_path,
                        candidate_contents,
                        file_location,
                        file_kind,
                        debug_id,
                    );
                } else if let Some(next) = candidates.pop_front() {
                    self.state = ElfLoadState::AwaitingDebugLinkCandidate {
                        primary,
                        primary_info,
                        file_location,
                        file_kind,
                        debuglink,
                        debug_id,
                        candidates,
                        pending: next,
                    };
                } else {
                    self.start_primary_dwp_or_advance(
                        file_location,
                        primary,
                        primary_info,
                        file_kind,
                    );
                }
            }
            ElfLoadState::AwaitingDebugLinkDwp {
                candidate_contents,
                file_location,
                file_kind,
                debug_id,
                pending: _pending,
            } => {
                let dwp = result.ok().map(FileContentsWrapper::new);
                self.state = ElfLoadState::Done(build_elf_symbol_map_for_debuglink_match::<H>(
                    file_location,
                    candidate_contents,
                    dwp,
                    file_kind,
                    debug_id,
                ));
            }
            ElfLoadState::AwaitingPrimaryDwp {
                primary,
                primary_info,
                file_location,
                file_kind,
                pending: _pending,
            } => {
                let dwp = result.ok().map(FileContentsWrapper::new);
                self.advance_to_supplementary_or_finalize(
                    primary,
                    primary_info,
                    dwp,
                    file_location,
                    file_kind,
                );
            }
            ElfLoadState::AwaitingSupplementaryCandidate {
                primary,
                primary_info,
                dwp,
                file_location,
                file_kind,
                debugaltlink,
                mut candidates,
                pending: _pending,
            } => {
                let matched = match result {
                    Ok(file) => {
                        let bytes = FileContentsWrapper::new(file);
                        if supplementary_build_id_matches(&bytes, &debugaltlink.build_id) {
                            Some(bytes)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                if let Some(supplementary) = matched {
                    self.state = ElfLoadState::Done(build_elf_symbol_map_with_supplementary::<H>(
                        file_location,
                        primary,
                        supplementary,
                        dwp,
                        file_kind,
                    ));
                } else if let Some(next) = candidates.pop_front() {
                    self.state = ElfLoadState::AwaitingSupplementaryCandidate {
                        primary,
                        primary_info,
                        dwp,
                        file_location,
                        file_kind,
                        debugaltlink,
                        candidates,
                        pending: next,
                    };
                } else {
                    self.state = ElfLoadState::Done(build_elf_symbol_map_no_supplementary::<H>(
                        file_location,
                        primary,
                        dwp,
                        file_kind,
                    ));
                }
            }
            ElfLoadState::NeedDebugLinkCandidates { .. }
            | ElfLoadState::NeedSupplementaryCandidates { .. } => {
                panic!("ElfLoad::provide called when awaiting candidates, not a file")
            }
            ElfLoadState::Done(_) | ElfLoadState::Poisoned => {
                panic!("ElfLoad::provide called when not awaiting a file")
            }
        }
    }

    /// Provide the candidate paths requested by
    /// `SymbolMapLoadStep::NeedDebugLinkCandidates` /
    /// `SymbolMapLoadStep::NeedSupplementaryCandidates`. An empty list is
    /// acceptable and means "no candidates"; the state machine will fall
    /// through to its next phase.
    pub fn provide_candidates(&mut self, candidates: Vec<H::FL>) {
        let state = std::mem::replace(&mut self.state, ElfLoadState::Poisoned);
        match state {
            ElfLoadState::NeedDebugLinkCandidates {
                primary,
                primary_info,
                file_location,
                file_kind,
                debuglink,
                debug_id,
            } => {
                let mut candidates: VecDeque<H::FL> = candidates.into();
                if let Some(pending) = candidates.pop_front() {
                    self.state = ElfLoadState::AwaitingDebugLinkCandidate {
                        primary,
                        primary_info,
                        file_location,
                        file_kind,
                        debuglink,
                        debug_id,
                        candidates,
                        pending,
                    };
                } else {
                    self.start_primary_dwp_or_advance(
                        file_location,
                        primary,
                        primary_info,
                        file_kind,
                    );
                }
            }
            ElfLoadState::NeedSupplementaryCandidates {
                primary,
                primary_info,
                dwp,
                file_location,
                file_kind,
                debugaltlink,
            } => {
                let mut candidates: VecDeque<H::FL> = candidates.into();
                if let Some(pending) = candidates.pop_front() {
                    self.state = ElfLoadState::AwaitingSupplementaryCandidate {
                        primary,
                        primary_info,
                        dwp,
                        file_location,
                        file_kind,
                        debugaltlink,
                        candidates,
                        pending,
                    };
                } else {
                    self.state = ElfLoadState::Done(build_elf_symbol_map_no_supplementary::<H>(
                        file_location,
                        primary,
                        dwp,
                        file_kind,
                    ));
                }
            }
            _ => panic!("ElfLoad::provide_candidates called when not awaiting candidates"),
        }
    }

    pub fn finish(self) -> Result<SymbolMap<H>, Error> {
        match self.state {
            ElfLoadState::Done(result) => result,
            _ => panic!("ElfLoad::finish called before reaching Done"),
        }
    }

    fn start_debuglink_or_advance(
        &mut self,
        file_location: H::FL,
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        file_kind: FileKind,
    ) {
        if let (Some(debuglink), Some(debug_id)) =
            (primary_info.debuglink.clone(), primary_info.debug_id)
        {
            self.state = ElfLoadState::NeedDebugLinkCandidates {
                primary,
                primary_info,
                file_location,
                file_kind,
                debuglink,
                debug_id,
            };
            return;
        }
        self.start_primary_dwp_or_advance(file_location, primary, primary_info, file_kind);
    }

    fn start_debuglink_dwp_with_path(
        &mut self,
        candidate_path: H::FL,
        candidate_contents: FileContentsWrapper<H::F>,
        file_location: H::FL,
        file_kind: FileKind,
        debug_id: DebugId,
    ) {
        match candidate_path.location_for_dwp() {
            Some(pending) => {
                self.state = ElfLoadState::AwaitingDebugLinkDwp {
                    candidate_contents,
                    file_location,
                    file_kind,
                    debug_id,
                    pending,
                };
            }
            None => {
                self.state = ElfLoadState::Done(build_elf_symbol_map_for_debuglink_match::<H>(
                    file_location,
                    candidate_contents,
                    None,
                    file_kind,
                    debug_id,
                ));
            }
        }
    }

    fn start_primary_dwp_or_advance(
        &mut self,
        file_location: H::FL,
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        file_kind: FileKind,
    ) {
        match file_location.location_for_dwp() {
            Some(pending) => {
                self.state = ElfLoadState::AwaitingPrimaryDwp {
                    primary,
                    primary_info,
                    file_location,
                    file_kind,
                    pending,
                };
            }
            None => {
                self.advance_to_supplementary_or_finalize(
                    primary,
                    primary_info,
                    None,
                    file_location,
                    file_kind,
                );
            }
        }
    }

    fn advance_to_supplementary_or_finalize(
        &mut self,
        primary: FileContentsWrapper<H::F>,
        primary_info: ElfPrimaryInfo,
        dwp: Option<FileContentsWrapper<H::F>>,
        file_location: H::FL,
        file_kind: FileKind,
    ) {
        if let Some(debugaltlink) = primary_info.debugaltlink.clone() {
            self.state = ElfLoadState::NeedSupplementaryCandidates {
                primary,
                primary_info,
                dwp,
                file_location,
                file_kind,
                debugaltlink,
            };
            return;
        }
        self.state = ElfLoadState::Done(build_elf_symbol_map_no_supplementary::<H>(
            file_location,
            primary,
            dwp,
            file_kind,
        ));
    }
}
