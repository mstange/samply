use std::borrow::Cow;

use debugid::DebugId;

use crate::{
    shared::{AddressInfo, DwoRef, FileLocation},
    FileAndPathHelper, FrameDebugInfo,
};

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo>;
    fn lookup_svma(&self, svma: u64) -> Option<AddressInfo>;
    fn lookup_offset(&self, offset: u64) -> Option<AddressInfo>;
}

pub trait SymbolMapTraitWithAsyncLookup<FC>: SymbolMapTrait {
    fn get_as_symbol_map(&self) -> &dyn SymbolMapTrait;
    fn lookup_frames_with_continuation(&self, svma: u64) -> FramesLookupWithContinuationResult<FC>;
}

pub trait GetInnerSymbolMap {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a);
}

pub trait GetInnerSymbolMapWithAsyncLookup<FC> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTraitWithAsyncLookup<FC> + 'a);
}

enum InnerSymbolMap<FC> {
    WithoutAddFile(Box<dyn GetInnerSymbolMap + Send>),
    WithAddFile(Box<dyn GetInnerSymbolMapWithAsyncLookup<FC> + Send>),
}

pub trait FramesLookupContinuation<'s, FC> {
    fn resume(&self, file_contents: Option<FC>) -> FramesLookupWithContinuationResult<'s, FC>;
}

pub enum FramesLookupWithContinuationResult<'s, FC> {
    Done(Option<Vec<FrameDebugInfo>>),
    NeedDwo(DwoRef, Box<dyn FramesLookupContinuation<'s, FC> + 's>),
}

pub struct SymbolMap<FL: FileLocation, FC> {
    debug_file_location: FL,
    inner: InnerSymbolMap<FC>,
}

impl<FL: FileLocation, FC> SymbolMap<FL, FC> {
    pub(crate) fn new_without(
        debug_file_location: FL,
        inner: Box<dyn GetInnerSymbolMap + Send>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithoutAddFile(inner),
        }
    }

    pub(crate) fn new_with(
        debug_file_location: FL,
        inner: Box<dyn GetInnerSymbolMapWithAsyncLookup<FC> + Send>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: InnerSymbolMap::WithAddFile(inner),
        }
    }

    fn inner(&self) -> &(dyn SymbolMapTrait + '_) {
        match &self.inner {
            InnerSymbolMap::WithoutAddFile(inner) => inner.get_inner_symbol_map(),
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map().get_as_symbol_map(),
        }
    }

    pub fn debug_file_location(&self) -> &FL {
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

    pub async fn lookup_frames_async<H: FileAndPathHelper<F = FC, FL = FL>>(
        &self,
        svma: u64,
        helper: &H,
    ) -> Option<Vec<FrameDebugInfo>> {
        let inner = match &self.inner {
            InnerSymbolMap::WithoutAddFile(_) => {
                println!("without add file");
                return None;
            }
            InnerSymbolMap::WithAddFile(inner) => inner.get_inner_symbol_map(),
        };
        let mut lookup_result = inner.lookup_frames_with_continuation(svma);
        loop {
            match lookup_result {
                FramesLookupWithContinuationResult::Done(frames) => return frames,
                FramesLookupWithContinuationResult::NeedDwo(dwo_ref, continuation) => {
                    println!("wanting to load dwo_ref {dwo_ref:?}");
                    let location = self.debug_file_location.location_for_dwo(dwo_ref);
                    let file_contents = match location {
                        Some(location) => helper.load_file(location).await.ok(),
                        None => None,
                    };
                    lookup_result = continuation.resume(file_contents);
                }
            }
        }
    }
}
