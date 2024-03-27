use std::borrow::Cow;

use debugid::DebugId;

use crate::shared::{AddressInfo, FileLocation};

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
}
