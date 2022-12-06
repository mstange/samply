use std::borrow::Cow;

use object::{File, ReadRef};
use yoke::{Yoke, Yokeable};

use crate::{
    debug_id_for_object,
    dwarf::Addr2lineContextData,
    path_mapper::PathMapper,
    shared::{AddressInfo, BasePath, SymbolMap, SymbolMapTrait, SymbolMapTypeErased},
    Error,
};

pub trait SymbolDataTrait {
    fn make_object_wrapper(&self) -> Result<Box<dyn ObjectWrapperTrait + '_>, Error>;
}

pub trait FunctionAddressesComputer<'data> {
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>;
}

pub struct ObjectData<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>> {
    object: File<'data, R>,
    function_addresses_computer: FAC,
    file_data: R,
    addr2line_context_data: Addr2lineContextData,
}

impl<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>> ObjectData<'data, R, FAC> {
    pub fn new(object: File<'data, R>, function_addresses_computer: FAC, file_data: R) -> Self {
        Self {
            object,
            function_addresses_computer,
            file_data,
            addr2line_context_data: Addr2lineContextData::new(),
        }
    }
}

pub trait ObjectWrapperTrait {
    fn make_symbol_map<'object>(
        &'object self,
        base_path: &BasePath,
    ) -> Result<SymbolMapTypeErased<'object>, Error>;
}

#[derive(Yokeable)]
pub struct ObjectWrapperTypeErased<'data>(Box<dyn ObjectWrapperTrait + 'data>);

impl<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>> ObjectWrapperTrait
    for ObjectData<'data, R, FAC>
{
    fn make_symbol_map<'file>(
        &'file self,
        base_path: &BasePath,
    ) -> Result<SymbolMapTypeErased<'file>, Error> {
        let (function_starts, function_ends) = self
            .function_addresses_computer
            .compute_function_addresses(&self.object);
        let debug_id = debug_id_for_object(&self.object)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;

        let symbol_map = SymbolMap::new(
            &self.object,
            self.file_data,
            debug_id,
            PathMapper::new(base_path),
            function_starts.as_deref(),
            function_ends.as_deref(),
            &self.addr2line_context_data,
        );
        let symbol_map = SymbolMapTypeErased(Box::new(symbol_map));
        Ok(symbol_map)
    }
}

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
