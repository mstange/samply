use std::borrow::Cow;

use debugid::DebugId;
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::error::Error;
use crate::shared::{AddressInfo, FileLocation};

pub struct SymbolMap<FL: FileLocation> {
    debug_file_location: FL,
    inner: Box<dyn GetInnerSymbolMap + Send>,
}

pub trait GetInnerSymbolMap {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a);
}

impl<FL: FileLocation> SymbolMap<FL> {
    pub(crate) fn new(debug_file_location: FL, inner: Box<dyn GetInnerSymbolMap + Send>) -> Self {
        Self {
            debug_file_location,
            inner,
        }
    }

    fn inner(&self) -> &(dyn SymbolMapTrait + '_) {
        self.inner.get_inner_symbol_map()
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

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo>;
    fn lookup_svma(&self, svma: u64) -> Option<AddressInfo>;
    fn lookup_offset(&self, offset: u64) -> Option<AddressInfo>;
}

pub trait SymbolMapDataOuterTrait {
    fn make_symbol_map_inner(&self) -> Result<SymbolMapInnerWrapper<'_>, Error>;
}

#[derive(Yokeable)]
pub struct SymbolMapInnerWrapper<'data>(pub Box<dyn SymbolMapTrait + Send + 'data>);

pub struct GenericSymbolMap<SMDO: SymbolMapDataOuterTrait>(
    Yoke<SymbolMapInnerWrapper<'static>, Box<SMDO>>,
);

impl<SMDO: SymbolMapDataOuterTrait + 'static> GenericSymbolMap<SMDO> {
    pub fn new(outer: SMDO) -> Result<Self, Error> {
        let outer_and_inner =
            Yoke::<SymbolMapInnerWrapper, _>::try_attach_to_cart(Box::new(outer), |outer| {
                outer.make_symbol_map_inner()
            })?;
        Ok(GenericSymbolMap(outer_and_inner))
    }
}

impl<SMDO: SymbolMapDataOuterTrait> GetInnerSymbolMap for GenericSymbolMap<SMDO> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a) {
        self.0.get().0.as_ref()
    }
}
