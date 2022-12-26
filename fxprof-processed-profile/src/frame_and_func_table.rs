use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::category::{
    Category, CategoryHandle, CategoryPairHandle, SerializableSubcategoryColumn, Subcategory,
};
use crate::fast_hash_map::FastHashMap;
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::resource_table::{ResourceIndex, ResourceTable};
use crate::serialization_helpers::{SerializableRange, SerializableSingleValueColumn};
use crate::string_table::StringIndex;
use crate::thread_string_table::ThreadStringTable;

#[derive(Debug, Clone, Default)]
pub struct FrameTableAndFuncTable {
    // We create one func for every frame.
    frame_addresses: Vec<Option<u32>>,
    frame_categories: Vec<CategoryHandle>,
    frame_subcategories: Vec<Subcategory>,
    func_names: Vec<ThreadInternalStringIndex>,
    func_resources: Vec<Option<ResourceIndex>>,

    // address -> frame index
    internal_frame_to_frame_index: FastHashMap<InternalFrame, usize>,
}

impl FrameTableAndFuncTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(
        &mut self,
        string_table: &mut ThreadStringTable,
        resource_table: &mut ResourceTable,
        global_libs: &GlobalLibTable,
        frame: InternalFrame,
    ) -> usize {
        let frame_addresses = &mut self.frame_addresses;
        let frame_categories = &mut self.frame_categories;
        let frame_subcategories = &mut self.frame_subcategories;
        let func_names = &mut self.func_names;
        let func_resources = &mut self.func_resources;
        *self
            .internal_frame_to_frame_index
            .entry(frame.clone())
            .or_insert_with(|| {
                let frame_index = frame_addresses.len();
                let (address, location_string_index, resource) = match frame.location {
                    InternalFrameLocation::UnknownAddress(address) => {
                        let location_string = format!("0x{:x}", address);
                        let s = string_table.index_for_string(&location_string);
                        (None, s, None)
                    }
                    InternalFrameLocation::AddressInLib(address, lib_index) => {
                        let location_string = format!("0x{:x}", address);
                        let s = string_table.index_for_string(&location_string);
                        let res =
                            resource_table.resource_for_lib(lib_index, global_libs, string_table);
                        (Some(address), s, Some(res))
                    }
                    InternalFrameLocation::Label(string_index) => (None, string_index, None),
                };
                let CategoryPairHandle(category, subcategory_index) = frame.category_pair;
                let subcategory = match subcategory_index {
                    Some(index) => Subcategory::Normal(index),
                    None => Subcategory::Other(category),
                };
                frame_addresses.push(address);
                frame_categories.push(category);
                frame_subcategories.push(subcategory);
                func_names.push(location_string_index);
                func_resources.push(resource);
                frame_index
            })
    }

    pub fn as_frame_table<'a>(&'a self, categories: &'a [Category]) -> impl Serialize + 'a {
        SerializableFrameTable {
            table: self,
            categories,
        }
    }
}

struct SerializableFrameTable<'a> {
    table: &'a FrameTableAndFuncTable,
    categories: &'a [Category],
}

impl<'a> Serialize for SerializableFrameTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.table.frame_addresses.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry(
            "address",
            &SerializableFrameTableAddressColumn(&self.table.frame_addresses),
        )?;
        map.serialize_entry("inlineDepth", &SerializableSingleValueColumn(0u32, len))?;
        map.serialize_entry("category", &self.table.frame_categories)?;
        map.serialize_entry(
            "subcategory",
            &SerializableSubcategoryColumn(&self.table.frame_subcategories, self.categories),
        )?;
        map.serialize_entry("func", &SerializableRange(len))?;
        map.serialize_entry("nativeSymbol", &SerializableSingleValueColumn((), len))?;
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
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameLocation {
    UnknownAddress(u64),
    AddressInLib(u32, GlobalLibIndex),
    Label(ThreadInternalStringIndex),
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ThreadInternalStringIndex(pub StringIndex);

impl Serialize for ThreadInternalStringIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl FrameTableAndFuncTable {
    pub fn as_func_table(&self) -> impl Serialize + '_ {
        SerializableFuncTable(self)
    }
}

struct SerializableFuncTable<'a>(&'a FrameTableAndFuncTable);

impl<'a> Serialize for SerializableFuncTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.0.frame_addresses.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("name", &self.0.func_names)?;
        map.serialize_entry("isJS", &SerializableSingleValueColumn(false, len))?;
        map.serialize_entry("relevantForJS", &SerializableSingleValueColumn(false, len))?;
        map.serialize_entry(
            "resource",
            &SerializableFuncTableResourceColumn(&self.0.func_resources),
        )?;
        map.serialize_entry("fileName", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("lineNumber", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("columnNumber", &SerializableSingleValueColumn((), len))?;
        map.end()
    }
}

struct SerializableFuncTableResourceColumn<'a>(&'a [Option<ResourceIndex>]);

impl<'a> Serialize for SerializableFuncTableResourceColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for resource in self.0 {
            match resource {
                Some(resource) => seq.serialize_element(&resource)?,
                None => seq.serialize_element(&-1)?,
            }
        }
        seq.end()
    }
}
