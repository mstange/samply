use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::category::{CategoryHandle, SubcategoryHandle, SubcategoryIndex};
use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::func_table::{FuncIndex, FuncKey, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::native_symbols::NativeSymbolIndex;
use crate::resource_table::ResourceTable;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::SourceLocation;

#[derive(Debug, Clone, Default)]
pub struct FrameTable {
    func_table: FuncTable,
    resource_table: ResourceTable,

    func_col: Vec<FuncIndex>,
    category_col: Vec<CategoryHandle>,
    subcategory_col: Vec<SubcategoryIndex>,
    line_col: Vec<Option<u32>>,
    column_col: Vec<Option<u32>>,
    address_col: Vec<Option<u32>>,
    native_symbol_col: Vec<Option<NativeSymbolIndex>>,
    inline_depth_col: Vec<u16>,

    frame_key_set: FastIndexSet<InternalFrame>,
}

impl FrameTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(
        &mut self,
        frame: InternalFrame,
        global_libs: &mut GlobalLibTable,
        string_table: &mut ProfileStringTable,
    ) -> usize {
        let (frame_index, is_new) = self.frame_key_set.insert_full(frame);

        if !is_new {
            return frame_index;
        }

        let func_key = frame.func_key();
        let func = self.func_table.index_for_func(
            func_key,
            &mut self.resource_table,
            global_libs,
            string_table,
        );

        self.func_col.push(func);
        let SubcategoryHandle(category, subcategory) = frame.subcategory;
        self.category_col.push(category);
        self.subcategory_col.push(subcategory);
        self.line_col.push(frame.source_location.line);
        self.column_col.push(frame.source_location.col);

        match frame.variant {
            InternalFrameVariant::Label => {
                self.address_col.push(None);
                self.native_symbol_col.push(None);
                self.inline_depth_col.push(0);
            }
            InternalFrameVariant::Native(NativeFrameData {
                lib,
                native_symbol,
                relative_address,
                inline_depth,
            }) => {
                global_libs.add_lib_used_rva(lib, relative_address);

                self.address_col.push(Some(relative_address));
                self.native_symbol_col.push(native_symbol);
                self.inline_depth_col.push(inline_depth);
            }
        }

        frame_index
    }

    pub fn contains_js_frame(&self) -> bool {
        self.func_table.contains_js_func()
    }

    pub fn get_serializable_tables(
        &self,
    ) -> (SerializableFrameTable<'_>, &'_ FuncTable, &'_ ResourceTable) {
        (
            SerializableFrameTable(self),
            &self.func_table,
            &self.resource_table,
        )
    }
}

pub struct SerializableFrameTable<'a>(&'a FrameTable);

impl Serialize for SerializableFrameTable<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SerializableFrameTable(table) = self;
        let len = table.func_col.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("func", &table.func_col)?;
        map.serialize_entry("category", &table.category_col)?;
        map.serialize_entry("subcategory", &table.subcategory_col)?;
        map.serialize_entry("line", &table.line_col)?;
        map.serialize_entry("column", &table.column_col)?;
        map.serialize_entry(
            "address",
            &SerializableFrameTableAddressColumn(&table.address_col),
        )?;
        map.serialize_entry("nativeSymbol", &table.native_symbol_col)?;
        map.serialize_entry("inlineDepth", &table.inline_depth_col)?;
        map.serialize_entry("innerWindowID", &SerializableSingleValueColumn(0, len))?;
        map.end()
    }
}

struct SerializableFrameTableAddressColumn<'a>(&'a [Option<u32>]);

impl Serialize for SerializableFrameTableAddressColumn<'_> {
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

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct InternalFrame {
    pub name: StringHandle,
    pub variant: InternalFrameVariant,
    pub subcategory: SubcategoryHandle,
    pub source_location: SourceLocation,
    pub flags: FrameFlags,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeFrameData {
    pub lib: GlobalLibIndex,
    pub native_symbol: Option<NativeSymbolIndex>,
    pub relative_address: u32,
    pub inline_depth: u16,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameVariant {
    Label,
    Native(NativeFrameData),
}

impl InternalFrame {
    pub fn func_key(&self) -> FuncKey {
        let InternalFrame {
            name,
            variant,
            flags,
            ..
        } = *self;
        let file_path = self.source_location.file_path;
        let lib = match variant {
            InternalFrameVariant::Label => None,
            InternalFrameVariant::Native(NativeFrameData { lib, .. }) => Some(lib),
        };
        FuncKey {
            name,
            file_path,
            lib,
            flags,
        }
    }
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameAddress {
    Unknown(u64),
    InLib(u32, GlobalLibIndex),
}
