use std::borrow::Cow;
use std::sync::Arc;

use debugid::DebugId;
use object::FileKind;

use crate::elf::ElfLoad;
use crate::error::Error;
use crate::macho::{self, DyldCacheLoad};
use crate::sans_io::{LoadStep, NeedsFiles, SymbolMapLoadStep};
use crate::shared::{
    AddressInfo, ExternalFileRef, FileContentsWrapper, FileLoadError, FileLocation, FrameDebugInfo,
    MultiArchDisambiguator,
};
use crate::{
    breakpad, jitdump, windows, ExternalFileAddressRef, FileTypes, FramesLookupResult,
    FunctionNameHandle, LookupAddress, SourceFilePath, SourceFilePathHandle, SymbolInfo,
    SymbolNameHandle, SyncAddressInfo,
};

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    /// Look up information for an address synchronously.
    ///
    /// If the information is known to be in an external file, and this file is
    /// already cached within this symbol map, then that cached information is
    /// consulted as part of this lookup_sync invocation. This method only returns
    /// `FramesLookupResult::External` if the caller actually needs to supply new
    /// file contents with a follow-up call to `try_lookup_external_with_file_contents`.
    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo>;

    fn resolve_function_name(&self, handle: FunctionNameHandle) -> Cow<'_, str>;
    fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str>;
    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_>;

    fn set_access_pattern_hint(&self, _hint: AccessPatternHint) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccessPatternHint {
    Arbitrary,

    /// Indicates that lookup calls will happen with addresses in ascending
    /// order. This lets the symbol map to save memory because it can discard
    /// cached information about functions other than the one that contains
    /// the current address, because those earlier functions cover lower
    /// addresses and their information will not be needed by higher addresses.
    SequentialLookup,
}

pub trait SymbolMapTraitWithExternalFileSupport<FC>: SymbolMapTrait {
    fn get_as_symbol_map(&self) -> &dyn SymbolMapTrait;
    fn try_lookup_external(&self, external: &ExternalFileAddressRef) -> Option<FramesLookupResult>;
    fn try_lookup_external_with_file_contents(
        &self,
        external: &ExternalFileAddressRef,
        file_contents: Option<FC>,
    ) -> Option<FramesLookupResult>;
}

pub trait GetInnerSymbolMap {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a);
}

pub trait GetInnerSymbolMapWithLookupFramesExt<FC> {
    fn get_inner_symbol_map<'a>(
        &'a self,
    ) -> &'a (dyn SymbolMapTraitWithExternalFileSupport<FC> + Send + Sync + 'a);
}

enum InnerSymbolMap<FC> {
    WithoutAddFile(Box<dyn GetInnerSymbolMap + Send + Sync>),
    WithAddFile(Box<dyn GetInnerSymbolMapWithLookupFramesExt<FC> + Send + Sync>),
    Direct(Arc<dyn SymbolMapTrait + Send + Sync>),
}

pub struct SymbolMap<FT: FileTypes> {
    debug_file_location: FT::FL,
    inner: InnerSymbolMap<FT::F>,
}

impl<FT: FileTypes> SymbolMap<FT> {
    pub(crate) fn new_plain(
        debug_file_location: FT::FL,
        inner: Box<dyn GetInnerSymbolMap + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithoutAddFile(inner),
        }
    }

    pub(crate) fn new_with_external_file_support(
        debug_file_location: FT::FL,
        inner: Box<dyn GetInnerSymbolMapWithLookupFramesExt<FT::F> + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithAddFile(inner),
        }
    }

    pub fn with_symbol_map_trait(
        debug_file_location: FT::FL,
        inner: Arc<dyn SymbolMapTrait + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::Direct(inner),
        }
    }

    fn inner(&self) -> &dyn SymbolMapTrait {
        match &self.inner {
            InnerSymbolMap::WithoutAddFile(inner) => inner.get_inner_symbol_map(),
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map().get_as_symbol_map(),
            InnerSymbolMap::Direct(inner) => inner.as_ref(),
        }
    }

    /// Returns a reference to the inner symbol map trait that supports external
    /// file lookups, if this symbol map was constructed with that capability.
    /// Used by the sans-IO lookup state machine.
    pub fn external_lookup_inner(
        &self,
    ) -> Option<&(dyn SymbolMapTraitWithExternalFileSupport<FT::F> + Send + Sync)> {
        match &self.inner {
            InnerSymbolMap::WithAddFile(inner) => Some(inner.get_inner_symbol_map()),
            InnerSymbolMap::WithoutAddFile(_) | InnerSymbolMap::Direct(_) => None,
        }
    }

    /// Apply newly-fetched file contents to a `FramesLookupResult::External`
    /// reference. The result is `None` if this `SymbolMap` doesn't support
    /// external lookups (or the lookup yielded no info), otherwise the next
    /// `FramesLookupResult` (which may itself be `External` if the chain
    /// continues).
    ///
    /// Pass `file_contents = None` if the file fetch failed; the symbol map
    /// will fall back to whatever it has cached.
    pub fn try_lookup_external_with_file_contents(
        &self,
        external: &ExternalFileAddressRef,
        file_contents: Option<FT::F>,
    ) -> Option<FramesLookupResult> {
        self.external_lookup_inner()?
            .try_lookup_external_with_file_contents(external, file_contents)
    }

    pub fn debug_file_location(&self) -> &FT::FL {
        &self.debug_file_location
    }

    pub fn debug_id(&self) -> debugid::DebugId {
        self.inner().debug_id()
    }

    pub fn symbol_count(&self) -> usize {
        self.inner().symbol_count()
    }

    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.inner().iter_symbols()
    }

    pub fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        self.inner().lookup_sync(address)
    }

    pub fn resolve_function_name(&self, handle: FunctionNameHandle) -> Cow<'_, str> {
        self.inner().resolve_function_name(handle)
    }

    pub fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str> {
        self.inner().resolve_symbol_name(handle)
    }

    pub fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        self.inner().resolve_source_file_path(handle)
    }

    pub fn set_access_pattern_hint(&self, hint: AccessPatternHint) {
        self.inner().set_access_pattern_hint(hint);
    }
}

impl<FT: FileTypes> SymbolMapTrait for SymbolMap<FT> {
    fn debug_id(&self) -> debugid::DebugId {
        self.debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.iter_symbols()
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        self.lookup_sync(address)
    }

    fn resolve_function_name(&self, handle: FunctionNameHandle) -> Cow<'_, str> {
        self.resolve_function_name(handle)
    }

    fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str> {
        self.resolve_symbol_name(handle)
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        self.resolve_source_file_path(handle)
    }

    fn set_access_pattern_hint(&self, hint: AccessPatternHint) {
        self.set_access_pattern_hint(hint);
    }
}

/// State machine that follows the chain of [`FramesLookupResult::External`]
/// references for a single address lookup, fetching one external file per
/// step, without performing any I/O itself.
///
/// Use [`LookupQuery::for_address`] for the equivalent of `SymbolMap::lookup`,
/// or [`LookupQuery::for_external`] for the equivalent of `SymbolMap::lookup_external`.
pub struct LookupQuery<'a, FT: FileTypes> {
    inner: Option<&'a (dyn SymbolMapTraitWithExternalFileSupport<FT::F> + Send + Sync + 'a)>,
    debug_file_location: &'a FT::FL,
    state: LookupState<FT>,
}

enum LookupState<FT: FileTypes> {
    NeedFile {
        external: ExternalFileAddressRef,
        location: FT::FL,
        kind: LookupKind,
    },
    Done(LookupOutput),
    Poisoned,
}

#[derive(Clone)]
enum LookupKind {
    /// The terminal frames will be wrapped in an `AddressInfo` together with this symbol.
    Address(SymbolInfo),
    /// The terminal frames will be returned directly.
    External,
}

/// The terminal value of a [`LookupQuery`]. Variant chosen by the constructor.
pub enum LookupOutput {
    /// Result of [`LookupQuery::for_address`]. Mirrors `SymbolMap::lookup`.
    Address(Option<AddressInfo>),
    /// Result of [`LookupQuery::for_external`]. Mirrors `SymbolMap::lookup_external`.
    External(Option<Vec<FrameDebugInfo>>),
}

impl<'a, FT: FileTypes> LookupQuery<'a, FT> {
    /// Build a state machine for the equivalent of `SymbolMap::lookup`.
    pub fn for_address(symbol_map: &'a SymbolMap<FT>, address: LookupAddress) -> Self {
        let address_info_sync = match symbol_map.lookup_sync(address) {
            Some(info) => info,
            None => return Self::done(symbol_map, LookupOutput::Address(None)),
        };
        let symbol = address_info_sync.symbol;
        let inner = symbol_map.external_lookup_inner();
        match (address_info_sync.frames, inner) {
            (Some(FramesLookupResult::Available(frames)), _) => Self::done(
                symbol_map,
                LookupOutput::Address(Some(AddressInfo {
                    symbol,
                    frames: Some(frames),
                })),
            ),
            (None, _) | (Some(FramesLookupResult::External(_)), None) => Self::done(
                symbol_map,
                LookupOutput::Address(Some(AddressInfo {
                    symbol,
                    frames: None,
                })),
            ),
            (Some(FramesLookupResult::External(external)), Some(inner)) => {
                let mut q = Self {
                    inner: Some(inner),
                    debug_file_location: symbol_map.debug_file_location(),
                    state: LookupState::Poisoned,
                };
                q.advance_chain(external, LookupKind::Address(symbol));
                q
            }
        }
    }

    /// Build a state machine for the equivalent of `SymbolMap::lookup_external`.
    pub fn for_external(symbol_map: &'a SymbolMap<FT>, external: &ExternalFileAddressRef) -> Self {
        let inner = match symbol_map.external_lookup_inner() {
            Some(inner) => inner,
            None => return Self::done(symbol_map, LookupOutput::External(None)),
        };
        let initial = inner.try_lookup_external(external);
        let mut q = Self {
            inner: Some(inner),
            debug_file_location: symbol_map.debug_file_location(),
            state: LookupState::Poisoned,
        };
        q.process_lookup_result(initial, LookupKind::External);
        q
    }

    fn done(symbol_map: &'a SymbolMap<FT>, output: LookupOutput) -> Self {
        Self {
            inner: None,
            debug_file_location: symbol_map.debug_file_location(),
            state: LookupState::Done(output),
        }
    }

    pub fn finish(self) -> LookupOutput {
        match self.state {
            LookupState::Done(output) => output,
            _ => panic!("LookupQuery::finish called before reaching Done"),
        }
    }

    fn process_lookup_result(
        &mut self,
        lookup_result: Option<FramesLookupResult>,
        kind: LookupKind,
    ) {
        match lookup_result {
            Some(FramesLookupResult::Available(frames)) => {
                self.state = LookupState::Done(finalize_lookup(kind, Some(frames)));
            }
            None => {
                self.state = LookupState::Done(finalize_lookup(kind, None));
            }
            Some(FramesLookupResult::External(new_external)) => {
                self.advance_chain(new_external, kind);
            }
        }
    }

    fn advance_chain(&mut self, mut external: ExternalFileAddressRef, kind: LookupKind) {
        loop {
            let maybe_location = match &external.file_ref {
                ExternalFileRef::MachoExternalObject { file_path } => self
                    .debug_file_location
                    .location_for_external_object_file(file_path),
                ExternalFileRef::ElfExternalDwo { comp_dir, path } => {
                    self.debug_file_location.location_for_dwo(comp_dir, path)
                }
            };
            match maybe_location {
                Some(location) => {
                    self.state = LookupState::NeedFile {
                        external,
                        location,
                        kind,
                    };
                    return;
                }
                None => {
                    let inner = self.inner.expect("advance_chain requires inner symbol map");
                    let lookup_result =
                        inner.try_lookup_external_with_file_contents(&external, None);
                    match lookup_result {
                        Some(FramesLookupResult::Available(frames)) => {
                            self.state = LookupState::Done(finalize_lookup(kind, Some(frames)));
                            return;
                        }
                        None => {
                            self.state = LookupState::Done(finalize_lookup(kind, None));
                            return;
                        }
                        Some(FramesLookupResult::External(new_external)) => {
                            external = new_external;
                        }
                    }
                }
            }
        }
    }
}

impl<FT: FileTypes> NeedsFiles<FT> for LookupQuery<'_, FT> {
    fn poll(&self) -> LoadStep<'_, FT::FL> {
        match &self.state {
            LookupState::NeedFile { location, .. } => LoadStep::NeedFile {
                location,
                required: false,
            },
            LookupState::Done(_) => LoadStep::Done,
            LookupState::Poisoned => unreachable!("invalid LookupQuery state"),
        }
    }

    fn provide(&mut self, result: Result<FT::F, FileLoadError>) {
        let (external, kind) = match std::mem::replace(&mut self.state, LookupState::Poisoned) {
            LookupState::NeedFile { external, kind, .. } => (external, kind),
            _ => panic!("LookupQuery::provide called when not awaiting a file"),
        };
        let inner = self
            .inner
            .expect("LookupQuery::provide requires inner symbol map");
        let file_contents = result.ok();
        let lookup_result = inner.try_lookup_external_with_file_contents(&external, file_contents);
        self.process_lookup_result(lookup_result, kind);
    }
}

fn finalize_lookup(kind: LookupKind, frames: Option<Vec<FrameDebugInfo>>) -> LookupOutput {
    match kind {
        LookupKind::Address(symbol) => LookupOutput::Address(Some(AddressInfo { symbol, frames })),
        LookupKind::External => LookupOutput::External(frames),
    }
}

/// Top-level dispatcher state machine for `SymbolManager::load_symbol_map_from_location`
/// and `SymbolManager::load_symbol_map_for_dyld_cache`.
///
/// Loads the primary file (or root dyld cache), sniffs its format, and
/// delegates to the appropriate sub-machine.
pub struct LoadSymbolMap<FT: FileTypes> {
    state: LoadSymbolMapState<FT>,
}

enum LoadSymbolMapState<FT: FileTypes> {
    AwaitingPrimary {
        file_location: FT::FL,
        disambiguator: Option<MultiArchDisambiguator>,
        pending: FT::FL,
    },
    Elf(ElfLoad<FT>),
    DyldCache {
        sm: DyldCacheLoad<FT>,
        dyld_cache_path: FT::FL,
    },
    /// PE found a PDB reference; awaiting the PDB. On any failure we fall back
    /// to a PE-only symbol map.
    AwaitingPdbFromBinary {
        primary: FileContentsWrapper<FT::F>,
        file_location: FT::FL,
        file_kind: FileKind,
        expected_debug_id: DebugId,
        pending: FT::FL,
    },
    /// Breakpad sym file with an optional symindex companion.
    AwaitingBreakpadSymindex {
        primary: FileContentsWrapper<FT::F>,
        file_location: FT::FL,
        pending: FT::FL,
    },
    Done(Result<SymbolMap<FT>, Error>),
    Poisoned,
}

impl<FT: FileTypes> LoadSymbolMap<FT> {
    /// Construct a state machine for loading a symbol map from a single file
    /// path. Mirrors `SymbolManager::load_symbol_map_from_location`.
    pub fn new(file_location: FT::FL, disambiguator: Option<MultiArchDisambiguator>) -> Self {
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
    pub fn for_dyld_cache(dyld_cache_path: FT::FL, dylib_path: String) -> Self {
        let sm = DyldCacheLoad::<FT>::new(dyld_cache_path.clone(), dylib_path);
        Self {
            state: LoadSymbolMapState::DyldCache {
                sm,
                dyld_cache_path,
            },
        }
    }

    pub fn poll(&self) -> SymbolMapLoadStep<'_, FT::FL> {
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

    pub fn provide(&mut self, result: Result<FT::F, FileLoadError>) {
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
                        self.state =
                            LoadSymbolMapState::Done(Err(Error::OpenFile(pending.to_string(), e)));
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
                    Ok(file) => windows::build_pdb_symbol_map_with_debug_id_check::<FT>(
                        FileContentsWrapper::new(file),
                        file_location.clone(),
                        expected_debug_id,
                    ),
                    Err(e) => Err(Error::OpenFile(pending.to_string(), e)),
                };
                self.state = LoadSymbolMapState::Done(match pdb_attempt {
                    Ok(symbol_map) => Ok(symbol_map),
                    Err(_) => {
                        windows::get_symbol_map_for_pe::<FT>(primary, file_kind, file_location)
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
    pub fn provide_candidates(&mut self, candidates: Vec<FT::FL>) {
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

    pub fn finish(self) -> Result<SymbolMap<FT>, Error> {
        match self.state {
            LoadSymbolMapState::Done(result) => result,
            _ => panic!("LoadSymbolMap::finish called before reaching Done"),
        }
    }

    fn dispatch_primary(
        &mut self,
        file_contents: FileContentsWrapper<FT::F>,
        file_location: FT::FL,
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
                                macho::get_symbol_map_for_fat_archive_member::<FT>(
                                    file_location,
                                    file_contents,
                                    member,
                                )
                            }),
                    );
                }
                FileKind::MachO32 | FileKind::MachO64 => {
                    self.state = LoadSymbolMapState::Done(macho::get_symbol_map_for_macho::<FT>(
                        file_location,
                        file_contents,
                    ));
                }
                FileKind::Pe32 | FileKind::Pe64 => {
                    match windows::pe_pdb_location::<FT>(file_kind, &file_contents, &file_location)
                    {
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
                                LoadSymbolMapState::Done(windows::get_symbol_map_for_pe::<FT>(
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
            self.state = LoadSymbolMapState::Done(windows::get_symbol_map_for_pdb::<FT>(
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
            self.state = LoadSymbolMapState::Done(jitdump::get_symbol_map_for_jitdump::<FT>(
                file_contents,
                file_location,
            ));
        } else {
            self.state = LoadSymbolMapState::Done(Err(Error::InvalidInputError(
                "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
            )));
        }
    }

    fn settle_elf(&mut self, sm: ElfLoad<FT>) {
        match sm.poll() {
            SymbolMapLoadStep::Done => {
                self.state = LoadSymbolMapState::Done(sm.finish());
            }
            _ => {
                self.state = LoadSymbolMapState::Elf(sm);
            }
        }
    }

    fn settle_dyld_cache(&mut self, sm: DyldCacheLoad<FT>, dyld_cache_path: FT::FL) {
        match sm.poll() {
            LoadStep::Done => {
                self.state = LoadSymbolMapState::Done(sm.finish().and_then(|file_data| {
                    macho::build_symbol_map_from_dyld_cache_file_data::<FT>(
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
