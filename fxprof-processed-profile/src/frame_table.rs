use std::io::Write;

use crate::category::SubcategoryHandle;
use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::func_table::{FuncKey, FuncTable};
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
        let mut flags_col = Vec::with_capacity(len);
        let mut func_col = Vec::with_capacity(len);
        let mut category_col = Vec::with_capacity(len);
        let mut subcategory_col = Vec::with_capacity(len);
        let mut line_col = Vec::with_capacity(len);
        let mut column_col = Vec::with_capacity(len);
        let mut address_col = Vec::with_capacity(len);
        let mut lib_col = Vec::with_capacity(len);
        let mut native_symbol_col = Vec::with_capacity(len);

        let mut func_table = FuncTable::default();
        let mut resource_table = ResourceTable::default();
        let mut source_table = SourceTable::default();

        for frame in &self.frame_key_set {
            let func_key = frame.func_key(&mut source_table, &mut resource_table);
            let func = func_table.index_for_func(func_key);

            // Every frame we emit has a category.
            let mut flags = FLAG_HAS_CATEGORY;
            let SubcategoryHandle(category, subcategory) = frame.subcategory;

            let line_val = if let Some(line) = frame.source_location.line {
                flags |= FLAG_HAS_LINE;
                line as i32
            } else {
                0
            };
            let col_val = if let Some(col) = frame.source_location.col {
                flags |= FLAG_HAS_COLUMN;
                col as i32
            } else {
                0
            };

            let (addr_val, lib_val, native_sym_val) = match frame.variant {
                InternalFrameVariant::Label => (0, 0, 0),
                InternalFrameVariant::Native(NativeFrameData {
                    lib,
                    native_symbol,
                    relative_address,
                    inline_depth,
                }) => {
                    flags |= FLAG_HAS_ADDRESS;
                    if inline_depth > 0 {
                        flags |= FLAG_IS_INLINED;
                    }
                    let native_sym_val = if let Some(native_symbol) = native_symbol {
                        flags |= FLAG_HAS_NATIVE_SYMBOL;
                        native_symbol.as_i32()
                    } else {
                        0
                    };
                    (relative_address, lib.as_i32(), native_sym_val)
                }
            };

            func_col.push(func.0);
            category_col.push(category.0);
            subcategory_col.push(subcategory.0);
            line_col.push(line_val);
            column_col.push(col_val);
            address_col.push(addr_val);
            lib_col.push(lib_val);
            native_symbol_col.push(native_sym_val);
            flags_col.push(flags);
        }

        let frame_table = FrameTable {
            flags_col,
            func_col,
            category_col,
            subcategory_col: SubcategoryColumn::new(subcategory_col),
            line_col,
            column_col,
            address_col,
            lib_col,
            native_symbol_col,
        };

        FrameInternerTables {
            frame_table,
            func_table,
            source_table,
            resource_table,
        }
    }
}

// The bits of the frame table's `flags` column, as defined by the processed
// profile format (added in version 71). Each `HAS_*` bit determines whether
// the value in the corresponding column is meaningful.
// The format also has a `HasOriginalLocation` bit at `1 << 6`, but this crate
// does not emit original locations yet - those are used by source maps which
// our API doesn't support yet.
const FLAG_IS_INLINED: u8 = 1 << 0;
const FLAG_HAS_ADDRESS: u8 = 1 << 1;
const FLAG_HAS_CATEGORY: u8 = 1 << 2;
const FLAG_HAS_NATIVE_SYMBOL: u8 = 1 << 3;
const FLAG_HAS_LINE: u8 = 1 << 4;
const FLAG_HAS_COLUMN: u8 = 1 << 5;

/// The `subcategory` column. The format allows this column to be 8 or 16 bits
/// wide; we only pay for 16 bits if some category has more than 256
/// subcategories.
enum SubcategoryColumn {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl SubcategoryColumn {
    fn new(values: Vec<u16>) -> Self {
        match values.iter().all(|v| *v <= u8::MAX as u16) {
            true => Self::U8(values.into_iter().map(|v| v as u8).collect()),
            false => Self::U16(values),
        }
    }
}

pub struct FrameTable {
    flags_col: Vec<u8>,
    func_col: Vec<i32>,
    category_col: Vec<u8>,
    subcategory_col: SubcategoryColumn,
    line_col: Vec<i32>,          // `0` if no line, see FLAG_HAS_LINE
    column_col: Vec<i32>,        // `0` if no column, see FLAG_HAS_COLUMN
    address_col: Vec<u32>,       // relative address, `0` if no address, see FLAG_HAS_ADDRESS
    lib_col: Vec<i32>,           // GlobalLibIndex, `0` if no lib, see FLAG_HAS_ADDRESS
    native_symbol_col: Vec<i32>, // NativeSymbolIndex, `0` if none, see FLAG_HAS_NATIVE_SYMBOL
}

impl FrameTable {
    pub(crate) fn write_json<'p, W: Write>(
        &'p self,
        w: &mut Writer<'_, 'p, W>,
    ) -> std::io::Result<()> {
        let len = self.func_col.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("flags")?;
            w.typed_array(&self.flags_col)?;
            w.name("func")?;
            w.typed_array(&self.func_col)?;
            w.name("category")?;
            w.typed_array(&self.category_col)?;
            w.name("subcategory")?;
            match &self.subcategory_col {
                SubcategoryColumn::U8(values) => w.typed_array(values)?,
                SubcategoryColumn::U16(values) => w.typed_array(values)?,
            }
            w.name("line")?;
            w.typed_array(&self.line_col)?;
            w.name("column")?;
            w.typed_array(&self.column_col)?;
            w.name("address")?;
            w.typed_array(&self.address_col)?;
            w.name("lib")?;
            w.typed_array(&self.lib_col)?;
            w.name("nativeSymbol")?;
            w.typed_array(&self.native_symbol_col)?;
            // We never have an innerWindowID; `0` means "no innerWindowID".
            w.name("innerWindowID")?;
            w.f64_array_from_iter(len, std::iter::repeat(0.0).take(len))?;
            // We never have original locations, so no frame has HAS_ORIGINAL_LOCATION.
            w.name("originalLocation")?;
            w.typed_array_from_iter(len, std::iter::repeat(0i32).take(len))
        })
    }
}

impl<'p> SplitOutObjectBody<'p> for &'p FrameTable {
    fn write_body<W: Write>(self, w: &mut Writer<'_, 'p, W>) -> std::io::Result<()> {
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
