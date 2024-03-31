use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use debugid::DebugId;

use crate::{
    external_file,
    shared::{AddressInfo, DwoRef, FileLocation},
    ExternalFileAddressRef, ExternalFileSymbolMap, FileAndPathHelper, FrameDebugInfo,
};

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo>;
    fn lookup_svma(&self, svma: u64) -> Option<AddressInfo>;
    fn lookup_offset(&self, offset: u64) -> Option<AddressInfo>;
}

pub trait SymbolMapTraitWithLookupFramesExt<FC>: SymbolMapTrait {
    fn get_as_symbol_map(&self) -> &dyn SymbolMapTrait;
    fn lookup_frames_again(&self, svma: u64) -> FramesLookupResult2;
    fn lookup_frames_more(
        &self,
        svma: u64,
        dwo_ref: &DwoRef,
        file_contents: Option<FC>,
    ) -> FramesLookupResult2;
}

pub trait GetInnerSymbolMap {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a);
}

pub trait GetInnerSymbolMapWithLookupFramesExt<FC> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTraitWithLookupFramesExt<FC> + 'a);
}

enum InnerSymbolMap<FC> {
    WithoutAddFile(Box<dyn GetInnerSymbolMap + Send + Sync>),
    WithAddFile(Box<dyn GetInnerSymbolMapWithLookupFramesExt<FC> + Send + Sync>),
}

pub enum FramesLookupResult2 {
    Done(Option<Vec<FrameDebugInfo>>),
    NeedDwo(DwoRef),
}

pub struct SymbolMap<H: FileAndPathHelper> {
    debug_file_location: H::FL,
    inner: InnerSymbolMap<H::F>,
    helper: Option<Arc<H>>,
    cached_external_file: Mutex<Option<ExternalFileSymbolMap<H::F>>>,
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
            cached_external_file: Mutex::new(None),
        }
    }

    pub(crate) fn new_with(
        debug_file_location: H::FL,
        inner: Box<dyn GetInnerSymbolMapWithLookupFramesExt<H::F> + Send + Sync>,
        helper: Arc<H>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithAddFile(inner),
            helper: Some(helper),
            cached_external_file: Mutex::new(None),
        }
    }

    fn inner(&self) -> &dyn SymbolMapTrait {
        match &self.inner {
            InnerSymbolMap::WithoutAddFile(inner) => inner.get_inner_symbol_map(),
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map().get_as_symbol_map(),
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

    pub fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo> {
        self.inner().lookup_relative_address(address)
    }

    pub fn lookup_svma(&self, svma: u64) -> Option<AddressInfo> {
        self.inner().lookup_svma(svma)
    }

    pub fn lookup_offset(&self, offset: u64) -> Option<AddressInfo> {
        self.inner().lookup_offset(offset)
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
        address: &ExternalFileAddressRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        {
            let cached_external_file = self.cached_external_file.lock().ok()?;
            match &*cached_external_file {
                Some(external_file) if external_file.is_same_file(&address.file_ref) => {
                    return external_file.lookup(&address.address_in_file);
                }
                _ => {}
            }
        }

        let helper = self.helper.as_deref()?;
        let external_file =
            external_file::load_external_file(helper, &self.debug_file_location, &address.file_ref)
                .await
                .ok()?;
        let lookup_result = external_file.lookup(&address.address_in_file);

        if let Ok(mut guard) = self.cached_external_file.lock() {
            *guard = Some(external_file);
        }
        lookup_result
    }

    pub async fn lookup_frames_async(&self, svma: u64) -> Option<Vec<FrameDebugInfo>> {
        let Some(helper) = self.helper.as_deref() else {
            return None;
        };
        let mut lookup_result = match &self.inner {
            InnerSymbolMap::WithoutAddFile(_) => {
                return None;
            }
            InnerSymbolMap::WithAddFile(inner) => {
                inner.get_inner_symbol_map().lookup_frames_again(svma)
            }
        };
        loop {
            match lookup_result {
                FramesLookupResult2::Done(frames) => return frames,
                FramesLookupResult2::NeedDwo(dwo_ref) => {
                    let location = self.debug_file_location.location_for_dwo(&dwo_ref);
                    let file_contents = match location {
                        Some(location) => helper.load_file(location).await.ok(),
                        None => None,
                    };
                    lookup_result = match &self.inner {
                        InnerSymbolMap::WithoutAddFile(_) => panic!(),
                        InnerSymbolMap::WithAddFile(inner) => inner
                            .get_inner_symbol_map()
                            .lookup_frames_more(svma, &dwo_ref, file_contents),
                    };
                }
            }
        }
    }
}
