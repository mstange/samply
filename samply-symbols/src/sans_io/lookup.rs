use crate::sans_io::{LoadStep, NeedsFiles};
use crate::shared::{
    AddressInfo, ExternalFileAddressRef, ExternalFileRef, FileLoadError, FileLocation, FileTypes,
    FrameDebugInfo, FramesLookupResult, LookupAddress, SymbolInfo,
};
use crate::symbol_map::{SymbolMap, SymbolMapTraitWithExternalFileSupport};

/// State machine that follows the chain of [`FramesLookupResult::External`]
/// references for a single address lookup, fetching one external file per
/// step, without performing any I/O itself.
///
/// Use [`LookupQuery::for_address`] for the equivalent of `SymbolMap::lookup`,
/// or [`LookupQuery::for_external`] for the equivalent of `SymbolMap::lookup_external`.
pub struct LookupQuery<'a, H: FileTypes> {
    inner: Option<&'a (dyn SymbolMapTraitWithExternalFileSupport<H::F> + Send + Sync + 'a)>,
    debug_file_location: &'a H::FL,
    state: LookupState<H>,
}

enum LookupState<H: FileTypes> {
    NeedFile {
        external: ExternalFileAddressRef,
        location: H::FL,
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

impl<'a, H: FileTypes> LookupQuery<'a, H> {
    /// Build a state machine for the equivalent of `SymbolMap::lookup`.
    pub fn for_address(symbol_map: &'a SymbolMap<H>, address: LookupAddress) -> Self {
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
    pub fn for_external(symbol_map: &'a SymbolMap<H>, external: &ExternalFileAddressRef) -> Self {
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

    fn done(symbol_map: &'a SymbolMap<H>, output: LookupOutput) -> Self {
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
                self.state = LookupState::Done(finalize(kind, Some(frames)));
            }
            None => {
                self.state = LookupState::Done(finalize(kind, None));
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
                            self.state = LookupState::Done(finalize(kind, Some(frames)));
                            return;
                        }
                        None => {
                            self.state = LookupState::Done(finalize(kind, None));
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

impl<H: FileTypes> NeedsFiles<H> for LookupQuery<'_, H> {
    fn poll(&self) -> LoadStep<'_, H::FL> {
        match &self.state {
            LookupState::NeedFile { location, .. } => LoadStep::NeedFile {
                location,
                required: false,
            },
            LookupState::Done(_) => LoadStep::Done,
            LookupState::Poisoned => unreachable!("invalid LookupQuery state"),
        }
    }

    fn provide(&mut self, result: Result<H::F, FileLoadError>) {
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

fn finalize(kind: LookupKind, frames: Option<Vec<FrameDebugInfo>>) -> LookupOutput {
    match kind {
        LookupKind::Address(symbol) => LookupOutput::Address(Some(AddressInfo { symbol, frames })),
        LookupKind::External => LookupOutput::External(frames),
    }
}
