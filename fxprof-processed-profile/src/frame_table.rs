use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::category::{CategoryHandle, SubcategoryHandle, SubcategoryIndex};
use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::func_table::{FuncIndex, FuncKey, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::native_symbols::NativeSymbolIndex;
use crate::resource_table::ResourceTable;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::string_table::StringHandle;
use crate::SourceLocation;

#[derive(Debug, Clone, Default)]
pub struct FrameInterner {
    frame_key_set: FastIndexSet<InternalFrame>,
    contains_js_frame: bool,
}

impl FrameInterner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(
        &mut self,
        frame: InternalFrame,
        global_libs: &mut GlobalLibTable,
    ) -> usize {
        let (frame_index, is_new) = self.frame_key_set.insert_full(frame);

        if is_new {
            if frame
                .flags
                .intersects(FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS)
            {
                self.contains_js_frame = true;
            }

            if let InternalFrameVariant::Native(NativeFrameData {
                lib,
                relative_address,
                ..
            }) = frame.variant
            {
                global_libs.add_lib_used_rva(lib, relative_address);
            }
        }
        frame_index
    }

    pub fn contains_js_frame(&self) -> bool {
        self.contains_js_frame
    }

    pub fn create_tables(&self) -> (FrameTable, FuncTable, ResourceTable) {
        let len = self.frame_key_set.len();
        let mut func_col = Vec::with_capacity(len);
        let mut category_col = Vec::with_capacity(len);
        let mut subcategory_col = Vec::with_capacity(len);
        let mut line_col = Vec::with_capacity(len);
        let mut column_col = Vec::with_capacity(len);
        let mut address_col = Vec::with_capacity(len);
        let mut native_symbol_col = Vec::with_capacity(len);
        let mut inline_depth_col = Vec::with_capacity(len);

        let mut func_table = FuncTable::default();
        let mut resource_table = ResourceTable::default();

        for frame in &self.frame_key_set {
            let func_key = frame.func_key();
            let func = func_table.index_for_func(func_key, &mut resource_table);

            func_col.push(func);
            let SubcategoryHandle(category, subcategory) = frame.subcategory;
            category_col.push(category);
            subcategory_col.push(subcategory);
            line_col.push(frame.source_location.line);
            column_col.push(frame.source_location.col);

            match frame.variant {
                InternalFrameVariant::Label => {
                    address_col.push(None);
                    native_symbol_col.push(None);
                    inline_depth_col.push(0);
                }
                InternalFrameVariant::Native(NativeFrameData {
                    native_symbol,
                    relative_address,
                    inline_depth,
                    ..
                }) => {
                    address_col.push(Some(relative_address));
                    native_symbol_col.push(native_symbol);
                    inline_depth_col.push(inline_depth);
                }
            }
        }

        let frame_table = FrameTable {
            func_col,
            category_col,
            subcategory_col,
            line_col,
            column_col,
            address_col,
            native_symbol_col,
            inline_depth_col,
        };

        (frame_table, func_table, resource_table)
    }
}

pub struct FrameTable {
    func_col: Vec<FuncIndex>,
    category_col: Vec<CategoryHandle>,
    subcategory_col: Vec<SubcategoryIndex>,
    line_col: Vec<Option<u32>>,
    column_col: Vec<Option<u32>>,
    address_col: Vec<Option<u32>>,
    native_symbol_col: Vec<Option<NativeSymbolIndex>>,
    inline_depth_col: Vec<u16>,
}

impl Serialize for FrameTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.func_col.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("func", &self.func_col)?;
        map.serialize_entry("category", &self.category_col)?;
        map.serialize_entry("subcategory", &self.subcategory_col)?;
        map.serialize_entry("line", &self.line_col)?;
        map.serialize_entry("column", &self.column_col)?;
        map.serialize_entry(
            "address",
            &SerializableFrameTableAddressColumn(&self.address_col),
        )?;
        map.serialize_entry("nativeSymbol", &self.native_symbol_col)?;
        map.serialize_entry("inlineDepth", &self.inline_depth_col)?;
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
