use debugid::DebugId;
use object::FileKind;

use crate::breakpad;
use crate::error::Error;
use crate::jitdump;
use crate::macho;
use crate::sans_io::dyld_cache_load::DyldCacheLoad;
use crate::sans_io::elf_load::ElfLoad;
use crate::sans_io::{LoadStep, NeedsFiles, SymbolMapLoadStep};
use crate::shared::{
    FileContentsWrapper, FileLoadError, FileLocation, FileTypes, MultiArchDisambiguator,
};
use crate::symbol_map::SymbolMap;
use crate::windows;

/// Top-level dispatcher state machine for `SymbolManager::load_symbol_map_from_location`
/// and `SymbolManager::load_symbol_map_for_dyld_cache`.
///
/// Loads the primary file (or root dyld cache), sniffs its format, and
/// delegates to the appropriate sub-machine.
pub struct LoadSymbolMap<H: FileTypes> {
    state: LoadSymbolMapState<H>,
}

enum LoadSymbolMapState<H: FileTypes> {
    AwaitingPrimary {
        file_location: H::FL,
        disambiguator: Option<MultiArchDisambiguator>,
        pending: H::FL,
    },
    Elf(ElfLoad<H>),
    DyldCache {
        sm: DyldCacheLoad<H>,
        dyld_cache_path: H::FL,
    },
    /// PE found a PDB reference; awaiting the PDB. On any failure we fall back
    /// to a PE-only symbol map.
    AwaitingPdbFromBinary {
        primary: FileContentsWrapper<H::F>,
        file_location: H::FL,
        file_kind: FileKind,
        expected_debug_id: DebugId,
        pending: H::FL,
    },
    /// Breakpad sym file with an optional symindex companion.
    AwaitingBreakpadSymindex {
        primary: FileContentsWrapper<H::F>,
        file_location: H::FL,
        pending: H::FL,
    },
    Done(Result<SymbolMap<H>, Error>),
    Poisoned,
}

impl<H: FileTypes> LoadSymbolMap<H> {
    /// Construct a state machine for loading a symbol map from a single file
    /// path. Mirrors `SymbolManager::load_symbol_map_from_location`.
    pub fn new(file_location: H::FL, disambiguator: Option<MultiArchDisambiguator>) -> Self {
        let pending = file_location.clone();
        Self {
            state: LoadSymbolMapState::AwaitingPrimary {
                file_location,
                disambiguator,
                pending,
            },
        }
    }

    /// Construct a state machine for loading a symbol map for a dylib in a
    /// dyld shared cache. Mirrors `SymbolManager::load_symbol_map_for_dyld_cache`.
    pub fn for_dyld_cache(dyld_cache_path: H::FL, dylib_path: String) -> Self {
        let sm = DyldCacheLoad::<H>::new(dyld_cache_path.clone(), dylib_path);
        Self {
            state: LoadSymbolMapState::DyldCache {
                sm,
                dyld_cache_path,
            },
        }
    }

    pub fn poll(&self) -> SymbolMapLoadStep<'_, H::FL> {
        match &self.state {
            LoadSymbolMapState::AwaitingPrimary { pending, .. } => SymbolMapLoadStep::NeedFile {
                location: pending,
                required: true,
            },
            LoadSymbolMapState::Elf(sm) => sm.poll(),
            LoadSymbolMapState::DyldCache { sm, .. } => match sm.poll() {
                LoadStep::NeedFile { location, required } => {
                    SymbolMapLoadStep::NeedFile { location, required }
                }
                LoadStep::Done => SymbolMapLoadStep::Done,
            },
            LoadSymbolMapState::AwaitingPdbFromBinary { pending, .. } => {
                SymbolMapLoadStep::NeedFile {
                    location: pending,
                    required: false,
                }
            }
            LoadSymbolMapState::AwaitingBreakpadSymindex { pending, .. } => {
                SymbolMapLoadStep::NeedFile {
                    location: pending,
                    required: false,
                }
            }
            LoadSymbolMapState::Done(_) => SymbolMapLoadStep::Done,
            LoadSymbolMapState::Poisoned => unreachable!("invalid LoadSymbolMap state"),
        }
    }

    pub fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, LoadSymbolMapState::Poisoned);
        match state {
            LoadSymbolMapState::AwaitingPrimary {
                file_location,
                disambiguator,
                pending,
            } => {
                let file = match result {
                    Ok(file) => file,
                    Err(e) => {
                        self.state = LoadSymbolMapState::Done(Err(
                            Error::HelperErrorDuringOpenFile(pending.to_string(), e),
                        ));
                        return;
                    }
                };
                self.dispatch_primary(FileContentsWrapper::new(file), file_location, disambiguator);
            }
            LoadSymbolMapState::Elf(mut sm) => {
                sm.provide(result);
                self.settle_elf(sm);
            }
            LoadSymbolMapState::DyldCache {
                mut sm,
                dyld_cache_path,
            } => {
                sm.provide(result);
                self.settle_dyld_cache(sm, dyld_cache_path);
            }
            LoadSymbolMapState::AwaitingPdbFromBinary {
                primary,
                file_location,
                file_kind,
                expected_debug_id,
                pending,
            } => {
                let pdb_attempt = match result {
                    Ok(file) => windows::build_pdb_symbol_map_with_debug_id_check::<H>(
                        FileContentsWrapper::new(file),
                        file_location.clone(),
                        expected_debug_id,
                    ),
                    Err(e) => Err(Error::HelperErrorDuringOpenFile(pending.to_string(), e)),
                };
                self.state = LoadSymbolMapState::Done(match pdb_attempt {
                    Ok(symbol_map) => Ok(symbol_map),
                    Err(_) => {
                        windows::get_symbol_map_for_pe::<H>(primary, file_kind, file_location)
                    }
                });
            }
            LoadSymbolMapState::AwaitingBreakpadSymindex {
                primary,
                file_location,
                pending: _pending,
            } => {
                let symindex = result.ok().map(FileContentsWrapper::new);
                self.state = LoadSymbolMapState::Done(
                    breakpad::get_symbol_map_for_breakpad_sym(primary, symindex)
                        .map(|sm| SymbolMap::new_plain(file_location, Box::new(sm))),
                );
            }
            LoadSymbolMapState::Done(_) | LoadSymbolMapState::Poisoned => {
                panic!("LoadSymbolMap::provide called when not awaiting a file")
            }
        }
    }

    /// Forward a `provide_candidates` call to the embedded ELF state machine.
    /// Panics if the current state is not the embedded ELF state.
    pub fn provide_candidates(&mut self, candidates: Vec<H::FL>) {
        match &mut self.state {
            LoadSymbolMapState::Elf(sm) => {
                sm.provide_candidates(candidates);
                let placeholder = LoadSymbolMapState::Poisoned;
                let LoadSymbolMapState::Elf(sm) = std::mem::replace(&mut self.state, placeholder)
                else {
                    unreachable!()
                };
                self.settle_elf(sm);
            }
            _ => panic!("LoadSymbolMap::provide_candidates called when not in Elf state"),
        }
    }

    pub fn finish(self) -> Result<SymbolMap<H>, Error> {
        match self.state {
            LoadSymbolMapState::Done(result) => result,
            _ => panic!("LoadSymbolMap::finish called before reaching Done"),
        }
    }

    fn dispatch_primary(
        &mut self,
        file_contents: FileContentsWrapper<H::F>,
        file_location: H::FL,
        disambiguator: Option<MultiArchDisambiguator>,
    ) {
        if let Ok(file_kind) = FileKind::parse(&file_contents) {
            match file_kind {
                FileKind::Elf32 | FileKind::Elf64 => {
                    let elf = ElfLoad::new(file_location, file_contents, file_kind);
                    self.settle_elf(elf);
                }
                FileKind::MachOFat32 | FileKind::MachOFat64 => {
                    self.state = LoadSymbolMapState::Done(
                        macho::get_fat_archive_member(&file_contents, file_kind, disambiguator)
                            .and_then(|member| {
                                macho::get_symbol_map_for_fat_archive_member::<H>(
                                    file_location,
                                    file_contents,
                                    member,
                                )
                            }),
                    );
                }
                FileKind::MachO32 | FileKind::MachO64 => {
                    self.state = LoadSymbolMapState::Done(macho::get_symbol_map_for_macho::<H>(
                        file_location,
                        file_contents,
                    ));
                }
                FileKind::Pe32 | FileKind::Pe64 => {
                    match windows::pe_pdb_location::<H>(file_kind, &file_contents, &file_location) {
                        Ok((pdb_location, expected_debug_id)) => {
                            self.state = LoadSymbolMapState::AwaitingPdbFromBinary {
                                primary: file_contents,
                                file_location,
                                file_kind,
                                expected_debug_id,
                                pending: pdb_location,
                            };
                        }
                        Err(_) => {
                            self.state =
                                LoadSymbolMapState::Done(windows::get_symbol_map_for_pe::<H>(
                                    file_contents,
                                    file_kind,
                                    file_location,
                                ));
                        }
                    }
                }
                _ => {
                    self.state = LoadSymbolMapState::Done(Err(Error::InvalidInputError(
                        "Input was Archive, Coff or Wasm format, which are unsupported for now",
                    )));
                }
            }
        } else if windows::is_pdb_file(&file_contents) {
            self.state = LoadSymbolMapState::Done(windows::get_symbol_map_for_pdb::<H>(
                file_contents,
                file_location,
            ));
        } else if breakpad::is_breakpad_file(&file_contents) {
            match file_location.location_for_breakpad_symindex() {
                Some(pending) => {
                    self.state = LoadSymbolMapState::AwaitingBreakpadSymindex {
                        primary: file_contents,
                        file_location,
                        pending,
                    };
                }
                None => {
                    self.state = LoadSymbolMapState::Done(
                        breakpad::get_symbol_map_for_breakpad_sym(file_contents, None)
                            .map(|sm| SymbolMap::new_plain(file_location, Box::new(sm))),
                    );
                }
            }
        } else if jitdump::is_jitdump_file(&file_contents) {
            self.state = LoadSymbolMapState::Done(jitdump::get_symbol_map_for_jitdump::<H>(
                file_contents,
                file_location,
            ));
        } else {
            self.state = LoadSymbolMapState::Done(Err(Error::InvalidInputError(
                "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
            )));
        }
    }

    fn settle_elf(&mut self, sm: ElfLoad<H>) {
        match sm.poll() {
            SymbolMapLoadStep::Done => {
                self.state = LoadSymbolMapState::Done(sm.finish());
            }
            _ => {
                self.state = LoadSymbolMapState::Elf(sm);
            }
        }
    }

    fn settle_dyld_cache(&mut self, sm: DyldCacheLoad<H>, dyld_cache_path: H::FL) {
        match sm.poll() {
            LoadStep::Done => {
                self.state = LoadSymbolMapState::Done(sm.finish().and_then(|file_data| {
                    macho::build_symbol_map_from_dyld_cache_file_data::<H>(
                        dyld_cache_path,
                        file_data,
                    )
                }));
            }
            _ => {
                self.state = LoadSymbolMapState::DyldCache {
                    sm,
                    dyld_cache_path,
                };
            }
        }
    }
}
