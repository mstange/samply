use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::category::{CategoryHandle, SubcategoryHandle, SubcategoryIndex};
use crate::fast_hash_map::FastHashMap;
use crate::frame::FrameFlags;
use crate::func_table::{FuncIndex, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::native_symbols::NativeSymbolIndex;
use crate::resource_table::ResourceTable;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::thread_string_table::{ThreadInternalStringIndex, ThreadStringTable};

#[derive(Debug, Clone, Default)]
pub struct FrameTable {
    name_col: Vec<ThreadInternalStringIndex>,
    category_col: Vec<CategoryHandle>,
    subcategory_col: Vec<SubcategoryIndex>,
    flags_col: Vec<FrameFlags>,
    file_col: Vec<Option<ThreadInternalStringIndex>>,
    line_col: Vec<Option<u32>>,
    column_col: Vec<Option<u32>>,
    lib_col: Vec<Option<GlobalLibIndex>>,
    address_col: Vec<Option<u32>>,
    native_symbol_col: Vec<Option<NativeSymbolIndex>>,
    inline_depth_col: Vec<u16>,
    frame_key_to_frame_index: FastHashMap<InternalFrameKey, usize>,
    contains_js_frame: bool,
}

impl FrameTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(
        &mut self,
        frame: InternalFrame,
        global_lib_index_to_thread_string_index: &mut FastHashMap<
            GlobalLibIndex,
            ThreadInternalStringIndex,
        >,
        global_libs: &mut GlobalLibTable,
        string_table: &mut ThreadStringTable,
    ) -> usize {
        if let Some(index) = self.frame_key_to_frame_index.get(&frame.key) {
            return *index;
        }

        let flags = frame.key.flags;

        let frame_index = self.name_col.len();
        let SubcategoryHandle(category, subcategory) = frame.key.subcategory;
        self.name_col.push(frame.name);
        self.category_col.push(category);
        self.subcategory_col.push(subcategory);
        self.flags_col.push(flags);

        match frame.key.variant {
            InternalFrameKeyVariant::Label {
                file_path,
                line,
                col,
                ..
            } => {
                self.file_col.push(file_path);
                self.line_col.push(line);
                self.column_col.push(col);
                self.lib_col.push(None);
                self.address_col.push(None);
                self.native_symbol_col.push(None);
                self.inline_depth_col.push(0);
            }
            InternalFrameKeyVariant::Native {
                lib,
                relative_address,
                inline_depth,
            } => {
                self.file_col.push(None);
                self.line_col.push(None);
                self.column_col.push(None);
                self.lib_col.push(Some(lib));
                self.address_col.push(Some(relative_address));
                self.native_symbol_col.push(frame.native_symbol);
                self.inline_depth_col.push(inline_depth);

                global_libs.add_lib_used_rva(lib, relative_address);

                global_lib_index_to_thread_string_index
                    .entry(lib)
                    .or_insert_with(|| {
                        let lib_name = &global_libs.get_lib(lib).unwrap().name;
                        string_table.index_for_string(lib_name)
                    });
            }
        }

        self.frame_key_to_frame_index.insert(frame.key, frame_index);

        if flags.intersects(FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS) {
            self.contains_js_frame = true;
        }

        frame_index
    }

    pub fn contains_js_frame(&self) -> bool {
        self.contains_js_frame
    }

    pub fn get_serializable_tables(
        &self,
        global_lib_index_to_thread_string_index: &FastHashMap<
            GlobalLibIndex,
            ThreadInternalStringIndex,
        >,
    ) -> (SerializableFrameTable<'_>, FuncTable, ResourceTable) {
        let mut func_table = FuncTable::new();
        let mut resource_table = ResourceTable::new();
        let func_col = self
            .name_col
            .iter()
            .cloned()
            .zip(self.flags_col.iter().cloned())
            .zip(self.lib_col.iter().cloned())
            .zip(self.file_col.iter().cloned())
            .map(|(((name, flags), lib), file)| {
                let resource = lib.map(|lib| {
                    resource_table.resource_for_lib(lib, global_lib_index_to_thread_string_index)
                });
                func_table.index_for_func(name, file, resource, flags)
            })
            .collect();
        (
            SerializableFrameTable(self, func_col),
            func_table,
            resource_table,
        )
    }
}

pub struct SerializableFrameTable<'a>(&'a FrameTable, Vec<FuncIndex>);

impl Serialize for SerializableFrameTable<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SerializableFrameTable(table, func_col) = self;
        let len = table.name_col.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry(
            "address",
            &SerializableFrameTableAddressColumn(&table.address_col),
        )?;
        map.serialize_entry("inlineDepth", &table.inline_depth_col)?;
        map.serialize_entry("category", &table.category_col)?;
        map.serialize_entry("subcategory", &table.subcategory_col)?;
        map.serialize_entry("func", func_col)?;
        map.serialize_entry("nativeSymbol", &table.native_symbol_col)?;
        map.serialize_entry("innerWindowID", &SerializableSingleValueColumn(0, len))?;
        map.serialize_entry("line", &table.line_col)?;
        map.serialize_entry("column", &table.column_col)?;
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

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct InternalFrame {
    pub key: InternalFrameKey,
    pub name: ThreadInternalStringIndex,
    pub native_symbol: Option<NativeSymbolIndex>, // only used when key.variant is InternalFrameKeyVariant::Native
    pub file_path: Option<ThreadInternalStringIndex>,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct InternalFrameKey {
    pub variant: InternalFrameKeyVariant,
    pub subcategory: SubcategoryHandle,
    pub flags: FrameFlags,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameKeyVariant {
    Label {
        name: ThreadInternalStringIndex,
        file_path: Option<ThreadInternalStringIndex>,
        line: Option<u32>,
        col: Option<u32>,
    },
    Native {
        lib: GlobalLibIndex,
        relative_address: u32,
        inline_depth: u16,
    },
}

impl InternalFrameKeyVariant {
    pub fn new_label(name: ThreadInternalStringIndex) -> Self {
        InternalFrameKeyVariant::Label {
            name,
            file_path: None,
            line: None,
            col: None,
        }
    }
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameAddress {
    Unknown(u64),
    InLib(u32, GlobalLibIndex),
}
