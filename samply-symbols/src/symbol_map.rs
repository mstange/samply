use std::borrow::Cow;
use std::sync::Arc;

use debugid::DebugId;

use crate::shared::LookupAddress;
use crate::{
    AddressInfo, ExternalFileAddressRef, ExternalFileRef, FileAndPathHelper, FileLocation,
    FrameDebugInfo, FramesLookupResult, SyncAddressInfo,
};

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo>;
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

pub struct SymbolMap<H: FileAndPathHelper> {
    debug_file_location: H::FL,
    inner: InnerSymbolMap<H::F>,
    helper: Option<Arc<H>>,
}

impl<H: FileAndPathHelper> SymbolMap<H> {
    pub(crate) fn new_plain(
        debug_file_location: H::FL,
        inner: Box<dyn GetInnerSymbolMap + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithoutAddFile(inner),
            helper: None,
        }
    }

    pub(crate) fn new_with_external_file_support(
        debug_file_location: H::FL,
        inner: Box<dyn GetInnerSymbolMapWithLookupFramesExt<H::F> + Send + Sync>,
        helper: Arc<H>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithAddFile(inner),
            helper: Some(helper),
        }
    }

    pub fn with_symbol_map_trait(
        debug_file_location: H::FL,
        inner: Arc<dyn SymbolMapTrait + Send + Sync>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::Direct(inner),
            helper: None,
        }
    }

    fn inner(&self) -> &dyn SymbolMapTrait {
        match &self.inner {
            InnerSymbolMap::WithoutAddFile(inner) => inner.get_inner_symbol_map(),
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map().get_as_symbol_map(),
            InnerSymbolMap::Direct(inner) => inner.as_ref(),
        }
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

    pub async fn lookup(&self, address: LookupAddress) -> Option<AddressInfo> {
        let address_info = self.inner().lookup_sync(address)?;
        let symbol = address_info.symbol;
        let (mut external, inner) = match (address_info.frames, &self.inner) {
            (Some(FramesLookupResult::Available(frames)), _) => {
                return Some(AddressInfo {
                    symbol,
                    frames: Some(frames),
                });
            }
            (None, _) | (_, InnerSymbolMap::WithoutAddFile(_)) | (_, InnerSymbolMap::Direct(_)) => {
                return Some(AddressInfo {
                    symbol,
                    frames: None,
                });
            }
            (Some(FramesLookupResult::External(external)), InnerSymbolMap::WithAddFile(inner)) => {
                (external, inner.get_inner_symbol_map())
            }
        };
        let helper = self.helper.as_deref()?;
        loop {
            let maybe_file_location = match &external.file_ref {
                ExternalFileRef::MachoExternalObject { file_path } => self
                    .debug_file_location
                    .location_for_external_object_file(file_path),
                ExternalFileRef::ElfExternalDwo { comp_dir, path } => {
                    self.debug_file_location.location_for_dwo(comp_dir, path)
                }
            };
            let file_contents = match maybe_file_location {
                Some(location) => helper.load_file(location).await.ok(),
                None => None,
            };
            let lookup_result =
                inner.try_lookup_external_with_file_contents(&external, file_contents);
            external = match lookup_result {
                Some(FramesLookupResult::Available(frames)) => {
                    return Some(AddressInfo {
                        symbol,
                        frames: Some(frames),
                    });
                }
                None => {
                    return Some(AddressInfo {
                        symbol,
                        frames: None,
                    });
                }
                Some(FramesLookupResult::External(external)) => external,
            };
        }
    }

    /// Resolve a debug info lookup for which `SymbolMap::lookup_*` returned a
    /// `FramesLookupResult::External`.
    ///
    /// This method is asynchronous because it may load a new external file.
    ///
    /// This keeps the most recent external file cached, so that repeated lookups
    /// for the same external file are fast.
    pub async fn lookup_external(
        &self,
        external: &ExternalFileAddressRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        let helper = self.helper.as_deref()?;
        let inner = match &self.inner {
            InnerSymbolMap::WithoutAddFile(_) | InnerSymbolMap::Direct(_) => return None,
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map(),
        };

        let mut lookup_result: Option<FramesLookupResult> = inner.try_lookup_external(external);
        loop {
            let external = match lookup_result {
                Some(FramesLookupResult::Available(frames)) => return Some(frames),
                None => return None,
                Some(FramesLookupResult::External(external)) => external,
            };
            let maybe_file_location = match &external.file_ref {
                ExternalFileRef::MachoExternalObject { file_path } => self
                    .debug_file_location
                    .location_for_external_object_file(file_path),
                ExternalFileRef::ElfExternalDwo { comp_dir, path } => {
                    self.debug_file_location.location_for_dwo(comp_dir, path)
                }
            };
            let file_contents = match maybe_file_location {
                Some(location) => helper.load_file(location).await.ok(),
                None => None,
            };
            lookup_result = inner.try_lookup_external_with_file_contents(&external, file_contents);
        }
    }
}
