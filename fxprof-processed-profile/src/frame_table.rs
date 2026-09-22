use std::hash::{BuildHasher, Hash, Hasher};
use std::io::Write;

use crate::category::SubcategoryHandle;
use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::func_table::{FuncKey, FuncTable};
use crate::global_lib_table::{GlobalLibIndex, UsedLibraryAddressesCollector};
use crate::native_symbols::NativeSymbolIndex;
use crate::resource_table::ResourceTable;
use crate::source_table::{SourceKey, SourceTable};
use crate::string_table::StringHandle;
use crate::writer::Writer;
use crate::{FrameHandle, SourceLocation};

/// Interns frames, in two levels.
///
/// The two levels are:
///
/// 1. One set of [`FrameTemplate`] items (everything except address), and
/// 2. One set of [`FrameCols`] items (template + address).
///
/// The point of this distinction is to reduce memory consumption for the
/// common case of having almost-duplicate frames which only differ in the
/// address. This happens in the presence of inlining:
///
/// Let's say you sample five instructions in a function called `inner` that
/// was inlined into a function called `outer`, with the call to `inner` at
/// file.cpp:123. Every sampled instruction gets two frames each: one for
/// `outer` and one for `inner`. The frames for `inner` may have different
/// line numbers, dependending on which code inside `inner` was responsible
/// for the sampled instruction. But the frames for `outer` will all have the
/// same line number, because there's only call to `inner`. Those frames for
/// `outer` will only differ in the instruction address.
#[derive(Debug, Clone, Default)]
pub struct FrameInterner {
    templates: FastIndexSet<FrameTemplate>,
    frames: ColumnarInterner<FrameCols, u32>,
    contains_js_frame: bool,
}

pub struct FrameInternerTables<'a> {
    pub frame_table: FrameTable<'a>,
    pub func_table: FuncTable,
    pub source_table: SourceTable,
    pub resource_table: ResourceTable,
}

impl FrameInterner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_frame(&mut self, frame: InternalFrame) -> FrameHandle {
        let (template, address) = frame.split();
        let (template_index, is_new_template) = self.templates.insert_full(template);

        if is_new_template
            && template
                .flags
                .intersects(FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS)
        {
            self.contains_js_frame = true;
        }

        let frame_index = self.frames.insert(FrameRow {
            template: template_index as u32,
            address,
        });
        FrameHandle(frame_index as i32)
    }

    pub fn gather_used_rvas(&self, collector: &mut UsedLibraryAddressesCollector) {
        let cols = self.frames.store();
        for (&template, &address) in cols.template.iter().zip(cols.address.iter()) {
            let template = &self.templates[template as usize];
            if let FrameTemplateVariant::Native(NativeTemplateData { lib, .. }) = template.variant {
                collector.add_lib_used_rva(lib, address);
            }
        }
    }

    pub fn into_frames(self) -> impl Iterator<Item = InternalFrame> {
        let templates: Vec<FrameTemplate> = self.templates.into_iter().collect();
        let cols = self.frames.into_store();
        cols.template
            .into_iter()
            .zip(cols.address)
            .map(move |(template, address)| templates[template as usize].with_address(address))
    }

    pub fn contains_js_frame(&self) -> bool {
        self.contains_js_frame
    }

    /// Build the func / source / resource tables, and a [`FrameTable`] view
    /// which expands templates into the frame table's columns as it writes.
    ///
    /// The per-template work here (in particular `index_for_func`) used to run
    /// once per frame; it now runs once per distinct code location.
    pub fn create_tables(&self) -> FrameInternerTables<'_> {
        let mut func_table = FuncTable::default();
        let mut resource_table = ResourceTable::default();
        let mut source_table = SourceTable::default();

        let mut per_template = Vec::with_capacity(self.templates.len());

        for template in &self.templates {
            let func_key = template.func_key(&mut source_table, &mut resource_table);
            let func = func_table.index_for_func(func_key);

            // Every frame we emit has a category.
            let mut flags = FLAG_HAS_CATEGORY;
            let SubcategoryHandle(category, subcategory) = template.subcategory;

            let line = if let Some(line) = template.source_location.line {
                flags |= FLAG_HAS_LINE;
                line as i32
            } else {
                0
            };
            let column = if let Some(col) = template.source_location.col {
                flags |= FLAG_HAS_COLUMN;
                col as i32
            } else {
                0
            };

            let (lib, native_symbol) = match template.variant {
                FrameTemplateVariant::Label => (0, 0),
                FrameTemplateVariant::Native(NativeTemplateData {
                    lib,
                    native_symbol,
                    inline_depth,
                }) => {
                    flags |= FLAG_HAS_ADDRESS;
                    if inline_depth > 0 {
                        flags |= FLAG_IS_INLINED;
                    }
                    let native_symbol = if let Some(native_symbol) = native_symbol {
                        flags |= FLAG_HAS_NATIVE_SYMBOL;
                        native_symbol.as_i32()
                    } else {
                        0
                    };
                    (lib.as_i32(), native_symbol)
                }
            };

            per_template.push(TemplateColumns {
                flags,
                func: func.0,
                category: category.0,
                subcategory: subcategory.0,
                line,
                column,
                lib,
                native_symbol,
            });
        }

        // The format allows the subcategory column to be 8 or 16 bits wide;
        // we only pay for 16 bits if some category has more than 256
        // subcategories.
        let subcategory_needs_u16 = per_template.iter().any(|t| t.subcategory > u8::MAX as u16);

        FrameInternerTables {
            frame_table: FrameTable {
                rows: self.frames.store(),
                per_template,
                subcategory_needs_u16,
            },
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

/// One (template, address) pair, i.e. one row of the frame table.
#[derive(Debug, Clone, Copy)]
struct FrameRow {
    template: u32,
    address: u32,
}

/// Columnar storage for the (template, address) pairs.
#[derive(Debug, Clone, Default)]
struct FrameCols {
    template: Vec<u32>,
    address: Vec<u32>,
}

impl ColumnarStore for FrameCols {
    type Row = FrameRow;

    fn len(&self) -> usize {
        self.template.len()
    }

    fn hash_row<H: BuildHasher>(row: &FrameRow, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.template.hash(&mut h);
        row.address.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, index: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.template[index].hash(&mut h);
        self.address[index].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, index: usize, row: &FrameRow) -> bool {
        self.template[index] == row.template && self.address[index] == row.address
    }

    fn push(&mut self, row: FrameRow) {
        self.template.push(row.template);
        self.address.push(row.address);
    }
}

/// The frame table column values which are the same for every frame that
/// shares a [`FrameTemplate`].
struct TemplateColumns {
    flags: u8,
    func: i32,
    category: u8,
    subcategory: u16,
    line: i32,
    column: i32,
    lib: i32,
    native_symbol: i32,
}

/// A view of the frame table which expands templates into columns on the fly
/// as it serializes, so that we never materialize all nine columns at once.
pub struct FrameTable<'a> {
    rows: &'a FrameCols,
    per_template: Vec<TemplateColumns>,
    subcategory_needs_u16: bool,
}

impl<'a> FrameTable<'a> {
    /// One column's worth of values, one per frame, looked up via the frame's
    /// template.
    fn col_iter<'s, N: Copy + 's>(
        &'s self,
        f: impl Fn(&TemplateColumns) -> N + 's,
    ) -> impl Iterator<Item = N> + 's {
        self.rows
            .template
            .iter()
            .map(move |&t| f(&self.per_template[t as usize]))
    }

    pub(crate) fn write_json<'p, W: Write>(
        &'p self,
        w: &mut Writer<'_, 'p, W>,
    ) -> std::io::Result<()> {
        let len = self.rows.template.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("flags")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.flags))?;
            w.name("func")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.func))?;
            w.name("category")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.category))?;
            w.name("subcategory")?;
            if self.subcategory_needs_u16 {
                w.typed_array_from_iter(len, self.col_iter(|t| t.subcategory))?;
            } else {
                w.typed_array_from_iter(len, self.col_iter(|t| t.subcategory as u8))?;
            }
            w.name("line")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.line))?;
            w.name("column")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.column))?;
            // The address column is already stored exactly as the format wants
            // it, so it can go out without a copy. `0` for frames with no
            // address, see FLAG_HAS_ADDRESS.
            w.name("address")?;
            w.typed_array(&self.rows.address)?;
            w.name("lib")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.lib))?;
            w.name("nativeSymbol")?;
            w.typed_array_from_iter(len, self.col_iter(|t| t.native_symbol))?;
            // We never have an innerWindowID; `0` means "no innerWindowID".
            w.name("innerWindowID")?;
            w.f64_array_from_iter(len, std::iter::repeat(0.0).take(len))?;
            // We never have original locations, so no frame has HAS_ORIGINAL_LOCATION.
            w.name("originalLocation")?;
            w.typed_array_from_iter(len, std::iter::repeat(0i32).take(len))
        })
    }
}

/// Everything about a frame except which instruction it points at.
///
/// Interned separately from the address, see [`FrameInterner`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FrameTemplate {
    pub name: StringHandle,
    pub variant: FrameTemplateVariant,
    pub subcategory: SubcategoryHandle,
    pub source_location: SourceLocation,
    pub flags: FrameFlags,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeTemplateData {
    pub lib: GlobalLibIndex,
    pub native_symbol: Option<NativeSymbolIndex>,
    pub inline_depth: u16,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum FrameTemplateVariant {
    Label,
    Native(NativeTemplateData),
}

/// A complete frame.
///
/// This is the type callers pass in and get back out, but we don't store
/// it. Instead, frames are stored in two pieces: [`FrameTemplate`] for
// everything except the address, and then [`FrameCols`] adds the address.
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
    fn split(self) -> (FrameTemplate, u32) {
        let InternalFrame {
            name,
            variant,
            subcategory,
            source_location,
            flags,
        } = self;
        // Label frames have no address. Pin it to 0 so that two label frames
        // with the same template always dedup to the same row.
        let (variant, address) = match variant {
            InternalFrameVariant::Label => (FrameTemplateVariant::Label, 0),
            InternalFrameVariant::Native(NativeFrameData {
                lib,
                native_symbol,
                relative_address,
                inline_depth,
            }) => (
                FrameTemplateVariant::Native(NativeTemplateData {
                    lib,
                    native_symbol,
                    inline_depth,
                }),
                relative_address,
            ),
        };
        let template = FrameTemplate {
            name,
            variant,
            subcategory,
            source_location,
            flags,
        };
        (template, address)
    }
}

impl FrameTemplate {
    fn with_address(self, address: u32) -> InternalFrame {
        let FrameTemplate {
            name,
            variant,
            subcategory,
            source_location,
            flags,
        } = self;
        let variant = match variant {
            FrameTemplateVariant::Label => InternalFrameVariant::Label,
            FrameTemplateVariant::Native(NativeTemplateData {
                lib,
                native_symbol,
                inline_depth,
            }) => InternalFrameVariant::Native(NativeFrameData {
                lib,
                native_symbol,
                relative_address: address,
                inline_depth,
            }),
        };
        InternalFrame {
            name,
            variant,
            subcategory,
            source_location,
            flags,
        }
    }

    pub fn func_key(
        &self,
        source_table: &mut SourceTable,
        resource_table: &mut ResourceTable,
    ) -> FuncKey {
        let FrameTemplate {
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
            FrameTemplateVariant::Label => None,
            FrameTemplateVariant::Native(NativeTemplateData { lib, .. }) => Some(lib),
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
