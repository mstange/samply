use std::borrow::Cow;
use std::sync::Arc;

use debugid::DebugId;

use crate::{
    ExternalFileAddressRef, FileTypes, FramesLookupResult, FunctionNameHandle, LookupAddress,
    SourceFilePath, SourceFilePathHandle, SymbolNameHandle, SyncAddressInfo,
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

pub struct SymbolMap<H: FileTypes> {
    debug_file_location: H::FL,
    inner: InnerSymbolMap<H::F>,
}

impl<H: FileTypes> SymbolMap<H> {
    pub(crate) fn new_plain(
        debug_file_location: H::FL,
        inner: Box<dyn GetInnerSymbolMap + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithoutAddFile(inner),
        }
    }

    pub(crate) fn new_with_external_file_support(
        debug_file_location: H::FL,
        inner: Box<dyn GetInnerSymbolMapWithLookupFramesExt<H::F> + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithAddFile(inner),
        }
    }

    pub fn with_symbol_map_trait(
        debug_file_location: H::FL,
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
    ) -> Option<&(dyn SymbolMapTraitWithExternalFileSupport<H::F> + Send + Sync)> {
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
        file_contents: Option<H::F>,
    ) -> Option<FramesLookupResult> {
        self.external_lookup_inner()?
            .try_lookup_external_with_file_contents(external, file_contents)
    }

    pub fn debug_file_location(&self) -> &H::FL {
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

impl<H: FileTypes> SymbolMapTrait for SymbolMap<H> {
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
