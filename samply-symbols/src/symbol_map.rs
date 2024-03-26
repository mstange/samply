use std::{borrow::Cow, sync::Mutex};

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
    WithoutAddFile(Box<dyn GetInnerSymbolMap + Send>),
    WithAddFile(Box<dyn GetInnerSymbolMapWithLookupFramesExt<FC> + Send>),
}

pub enum FramesLookupResult2 {
    Done(Option<Vec<FrameDebugInfo>>),
    NeedDwo(DwoRef),
}

pub struct SymbolMap<FL: FileLocation, FC> {
    debug_file_location: FL,
    inner: Mutex<InnerSymbolMap<FC>>,
}

impl<FL: FileLocation, FC> SymbolMap<FL, FC> {
    pub(crate) fn new_without(
        debug_file_location: FL,
        inner: Box<dyn GetInnerSymbolMap + Send>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: Mutex::new(InnerSymbolMap::WithoutAddFile(inner)),
        }
    }

    pub(crate) fn new_with(
        debug_file_location: FL,
        inner: Box<dyn GetInnerSymbolMapWithLookupFramesExt<FC> + Send>,
    ) -> Self {
        Self {
            debug_file_location,
            inner: Mutex::new(InnerSymbolMap::WithAddFile(inner)),
        }
    }

    fn with_inner<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&dyn SymbolMapTrait) -> R,
    {
        match &*self.inner.lock().unwrap() {
            InnerSymbolMap::WithoutAddFile(inner) => f(inner.get_inner_symbol_map()),
            InnerSymbolMap::WithAddFile(inner) => {
                f(inner.get_inner_symbol_map().get_as_symbol_map())
            }
        }
    }

    pub fn debug_file_location(&self) -> &FL {
        &self.debug_file_location
    }

    pub fn debug_id(&self) -> debugid::DebugId {
        self.with_inner(|inner| inner.debug_id())
    }

    pub fn symbol_count(&self) -> usize {
        self.with_inner(|inner| inner.symbol_count())
    }

    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let vec = self.with_inner(|inner| {
            let vec: Vec<_> = inner
                .iter_symbols()
                .map(|(addr, s)| (addr, s.to_string()))
                .collect();
            vec
        });
        Box::new(vec.into_iter().map(|(addr, s)| (addr, Cow::Owned(s))))
    }

    pub fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo> {
        self.with_inner(|inner| inner.lookup_relative_address(address))
    }

    pub fn lookup_svma(&self, svma: u64) -> Option<AddressInfo> {
        self.with_inner(|inner| inner.lookup_svma(svma))
    }

    pub fn lookup_offset(&self, offset: u64) -> Option<AddressInfo> {
        self.with_inner(|inner| inner.lookup_offset(offset))
    }

    pub async fn lookup_frames_async<H: FileAndPathHelper<F = FC, FL = FL>>(
        &self,
        svma: u64,
        helper: &H,
    ) -> Option<Vec<FrameDebugInfo>> {
        let mut lookup_result = match &*self.inner.lock().unwrap() {
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
                    lookup_result = match &*self.inner.lock().unwrap() {
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
