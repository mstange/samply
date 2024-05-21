use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::category::{
    Category, CategoryHandle, CategoryPairHandle, SerializableSubcategoryColumn, Subcategory,
};
use crate::fast_hash_map::FastHashMap;
use crate::frame::FrameFlags;
use crate::func_table::{FuncIndex, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::native_symbols::{NativeSymbolIndex, NativeSymbols};
use crate::resource_table::ResourceTable;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::thread_string_table::{ThreadInternalStringIndex, ThreadStringTable};

#[derive(Debug, Clone, Default)]
pub struct FrameTable {
    addresses: Vec<Option<u32>>,
    categories: Vec<CategoryHandle>,
    subcategories: Vec<Subcategory>,
    funcs: Vec<FuncIndex>,
    native_symbols: Vec<Option<NativeSymbolIndex>>,
    internal_frame_to_frame_index: FastHashMap<InternalFrame, usize>,
}

impl FrameTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(
        &mut self,
        string_table: &mut ThreadStringTable,
        resource_table: &mut ResourceTable,
        func_table: &mut FuncTable,
        native_symbol_table: &mut NativeSymbols,
        global_libs: &mut GlobalLibTable,
        frame: InternalFrame,
    ) -> usize {
        let addresses = &mut self.addresses;
        let funcs = &mut self.funcs;
        let native_symbols = &mut self.native_symbols;
        let categories = &mut self.categories;
        let subcategories = &mut self.subcategories;
        *self
            .internal_frame_to_frame_index
            .entry(frame.clone())
            .or_insert_with(|| {
                let frame_index = addresses.len();
                let (address, location_string_index, native_symbol, resource) = match frame.location
                {
                    InternalFrameLocation::UnknownAddress(address) => {
                        let location_string = format!("0x{address:x}");
                        let s = string_table.index_for_string(&location_string);
                        (None, s, None, None)
                    }
                    InternalFrameLocation::AddressInLib(address, lib_index) => {
                        let res =
                            resource_table.resource_for_lib(lib_index, global_libs, string_table);
                        let lib = global_libs.get_lib(lib_index).unwrap();
                        let native_symbol_and_name =
                            lib.symbol_table.as_deref().and_then(|symbol_table| {
                                let symbol = symbol_table.lookup(address)?;
                                Some(
                                    native_symbol_table.symbol_index_and_string_index_for_symbol(
                                        lib_index,
                                        symbol,
                                        string_table,
                                    ),
                                )
                            });
                        let (native_symbol, s) = match native_symbol_and_name {
                            Some((native_symbol, name_string_index)) => {
                                (Some(native_symbol), name_string_index)
                            }
                            None => {
                                // This isn't in the pre-provided symbol table, and we know it's in a library.
                                global_libs.add_lib_used_rva(lib_index, address);

                                let location_string = format!("0x{address:x}");
                                (None, string_table.index_for_string(&location_string))
                            }
                        };
                        (Some(address), s, native_symbol, Some(res))
                    }
                    InternalFrameLocation::Label(string_index) => (None, string_index, None, None),
                };
                let func_index =
                    func_table.index_for_func(location_string_index, resource, frame.flags);
                let CategoryPairHandle(category, subcategory_index) = frame.category_pair;
                let subcategory = match subcategory_index {
                    Some(index) => Subcategory::Normal(index),
                    None => Subcategory::Other(category),
                };
                addresses.push(address);
                categories.push(category);
                subcategories.push(subcategory);
                funcs.push(func_index);
                native_symbols.push(native_symbol);
                frame_index
            })
    }

    pub fn as_serializable<'a>(&'a self, categories: &'a [Category]) -> impl Serialize + 'a {
        SerializableFrameTable {
            table: self,
            categories,
        }
    }
}

struct SerializableFrameTable<'a> {
    table: &'a FrameTable,
    categories: &'a [Category],
}

impl<'a> Serialize for SerializableFrameTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.table.addresses.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry(
            "address",
            &SerializableFrameTableAddressColumn(&self.table.addresses),
        )?;
        map.serialize_entry("inlineDepth", &SerializableSingleValueColumn(0u32, len))?;
        map.serialize_entry("category", &self.table.categories)?;
        map.serialize_entry(
            "subcategory",
            &SerializableSubcategoryColumn(&self.table.subcategories, self.categories),
        )?;
        map.serialize_entry("func", &self.table.funcs)?;
        map.serialize_entry("nativeSymbol", &self.table.native_symbols)?;
        map.serialize_entry("innerWindowID", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("implementation", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("line", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("column", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("optimizations", &SerializableSingleValueColumn((), len))?;
        map.end()
    }
}

struct SerializableFrameTableAddressColumn<'a>(&'a [Option<u32>]);

impl<'a> Serialize for SerializableFrameTableAddressColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for address in self.0 {
            match address {
                Some(address) => seq.serialize_element(&address)?,
                None => seq.serialize_element(&-1)?,
            }
        }
        seq.end()
    }
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct InternalFrame {
    pub location: InternalFrameLocation,
    pub category_pair: CategoryPairHandle,
    pub flags: FrameFlags,
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameLocation {
    UnknownAddress(u64),
    AddressInLib(u32, GlobalLibIndex),
    Label(ThreadInternalStringIndex),
}
