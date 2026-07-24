use std::io::Write;

use crate::category::{CategoryHandle, SubcategoryHandle, SubcategoryIndex};
use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::func_table::{FuncIndex, FuncKey, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, UsedLibraryAddressesCollector};
use crate::native_symbols::NativeSymbolIndex;
use crate::resource_table::ResourceTable;
use crate::source_table::{SourceKey, SourceTable};
use crate::string_table::StringHandle;
use crate::writer::{SplitOutObjectBody, Writer};
use crate::{FrameHandle, SourceLocation};

#[derive(Debug, Clone, Default)]
pub struct FrameInterner {
    frame_key_set: FastIndexSet<InternalFrame>,
    contains_js_frame: bool,
}

pub struct FrameInternerTables {
    pub frame_table: FrameTable,
    pub func_table: FuncTable,
    pub source_table: SourceTable,
    pub resource_table: ResourceTable,
}

impl FrameInterner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(&mut self, frame: InternalFrame) -> FrameHandle {
        let (frame_index, is_new) = self.frame_key_set.insert_full(frame);

        if is_new
            && frame
                .flags
                .intersects(FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS)
        {
            self.contains_js_frame = true;
        }
        FrameHandle(frame_index as i32)
    }

    pub fn gather_used_rvas(&self, collector: &mut UsedLibraryAddressesCollector) {
        for frame in &self.frame_key_set {
            if let InternalFrameVariant::Native(NativeFrameData {
                lib,
                relative_address,
                ..
            }) = frame.variant
            {
                collector.add_lib_used_rva(lib, relative_address);
            }
        }
    }

    pub fn into_frames(self) -> impl Iterator<Item = InternalFrame> {
        self.frame_key_set.into_iter()
    }

    pub fn contains_js_frame(&self) -> bool {
        self.contains_js_frame
    }

    pub fn create_tables(&self) -> FrameInternerTables {
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
        let mut source_table = SourceTable::default();

        for frame in &self.frame_key_set {
            let func_key = frame.func_key(&mut source_table, &mut resource_table);
            let func = func_table.index_for_func(func_key);

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

        FrameInternerTables {
            frame_table,
            func_table,
            source_table,
            resource_table,
        }
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

impl FrameTable {
    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let len = self.func_col.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("func")?;
            w.array(|w| {
                for f in &self.func_col {
                    f.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("category")?;
            w.array(|w| {
                for c in &self.category_col {
                    c.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("subcategory")?;
            w.array(|w| {
                for s in &self.subcategory_col {
                    s.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("line")?;
            w.optional_number_array(&self.line_col)?;
            w.name("column")?;
            w.optional_number_array(&self.column_col)?;
            w.name("address")?;
            w.array(|w| {
                for a in &self.address_col {
                    match a {
                        Some(a) => w.number_value(*a)?,
                        None => w.number_value(-1)?,
                    }
                }
                Ok(())
            })?;
            w.name("nativeSymbol")?;
            w.array(|w| {
                for n in &self.native_symbol_col {
                    NativeSymbolIndex::write_optional(*n, w)?;
                }
                Ok(())
            })?;
            w.name("inlineDepth")?;
            w.number_array(&self.inline_depth_col)?;
            w.name("innerWindowID")?;
            w.array(|w| {
                for _ in 0..len {
                    w.number_value(0u32)?;
                }
                Ok(())
            })?;
            w.name("originalLocation")?;
            w.null_array(len)
        })
    }
}

impl SplitOutObjectBody for &FrameTable {
    fn write_body<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        self.write_json(w)
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
    pub fn func_key(
        &self,
        source_table: &mut SourceTable,
        resource_table: &mut ResourceTable,
    ) -> FuncKey {
        let InternalFrame {
            name,
            variant,
            flags,
            ..
        } = *self;
        let SourceLocation {
            file_path,
            function_start_line,
            function_start_col,
            ..
        } = self.source_location;
        let source = file_path.map(|file_path| {
            source_table.index_for_source(SourceKey {
                id: None,
                file_path,
                start_line: 1,
                start_column: 1,
                source_map_url: None,
            })
        });
        let lib = match variant {
            InternalFrameVariant::Label => None,
            InternalFrameVariant::Native(NativeFrameData { lib, .. }) => Some(lib),
        };
        let resource = lib.map(|lib| resource_table.resource_for_lib(lib));
        FuncKey {
            name,
            source,
            start_line: function_start_line,
            start_column: function_start_col,
            resource,
            flags,
        }
    }
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum InternalFrameAddress {
    Unknown(u64),
    InLib(u32, GlobalLibIndex),
}
