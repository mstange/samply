use std::borrow::Cow;

use debugid::DebugId;
use yoke::{Yoke, Yokeable};

use crate::{
    shared::{AddressInfo, BasePath},
    Error,
};

pub trait SymbolDataTrait {
    fn make_object_wrapper(&self) -> Result<Box<dyn ObjectWrapperTrait + '_>, Error>;
}

pub trait ObjectWrapperTrait {
    fn make_symbol_map<'object>(
        &'object self,
        base_path: &BasePath,
    ) -> Result<SymbolMapTypeErased<'object>, Error>;
}

#[derive(Yokeable)]
pub struct ObjectWrapperTypeErased<'data>(Box<dyn ObjectWrapperTrait + 'data>);

struct SymbolMapDataWithObject<SMD: SymbolDataTrait>(
    Yoke<ObjectWrapperTypeErased<'static>, Box<SMD>>,
);

pub struct GenericSymbolMap<SMD: SymbolDataTrait>(
    Yoke<SymbolMapTypeErased<'static>, Box<SymbolMapDataWithObject<SMD>>>,
);

impl<SMD: SymbolDataTrait> GenericSymbolMap<SMD> {
    pub fn new(owner: SMD, base_path: &BasePath) -> Result<Self, Error> {
        let owner_with_object = SymbolMapDataWithObject(
            Yoke::<ObjectWrapperTypeErased<'static>, _>::try_attach_to_cart(
                Box::new(owner),
                |owner| owner.make_object_wrapper().map(ObjectWrapperTypeErased),
            )?,
        );
        let owner_with_symbol_map = Yoke::<SymbolMapTypeErased, _>::try_attach_to_cart(
            Box::new(owner_with_object),
            |owner_with_object| {
                let object = owner_with_object.0.get();
                object.0.make_symbol_map(base_path)
            },
        )?;
        Ok(GenericSymbolMap(owner_with_symbol_map))
    }
}

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn to_map(&self) -> Vec<(u32, String)>;

    fn lookup(&self, address: u32) -> Option<AddressInfo>;
}

#[derive(Yokeable)]
pub struct SymbolMapTypeErased<'data>(pub Box<dyn SymbolMapTrait + 'data>);

impl<'data> SymbolMapTypeErased<'data> {
    pub fn debug_id(&self) -> debugid::DebugId {
        self.0.debug_id()
    }

    pub fn symbol_count(&self) -> usize {
        self.0.symbol_count()
    }

    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.iter_symbols()
    }

    pub fn to_map(&self) -> Vec<(u32, String)> {
        self.0.to_map()
    }

    pub fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.lookup(address)
    }
}

pub struct SymbolMapTypeErasedOwned(pub Box<dyn SymbolMapTrait>);

impl SymbolMapTypeErasedOwned {
    pub fn debug_id(&self) -> debugid::DebugId {
        self.0.debug_id()
    }

    pub fn symbol_count(&self) -> usize {
        self.0.symbol_count()
    }

    #[allow(unused)]
    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.iter_symbols()
    }

    pub fn to_map(&self) -> Vec<(u32, String)> {
        self.0.to_map()
    }

    pub fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.lookup(address)
    }
}

impl<SMD: SymbolDataTrait> SymbolMapTrait for GenericSymbolMap<SMD> {
    fn debug_id(&self) -> debugid::DebugId {
        self.0.get().debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.0.get().symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.get().iter_symbols()
    }

    fn to_map(&self) -> Vec<(u32, String)> {
        self.0.get().to_map()
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.get().lookup(address)
    }
}
