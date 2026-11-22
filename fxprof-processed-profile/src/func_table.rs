use std::hash::{BuildHasher, Hash, Hasher};
use std::io::Write;

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::frame::FrameFlags;
use crate::resource_table::ResourceIndex;
use crate::source_table::SourceIndex;
use crate::string_table::StringHandle;
use crate::writer::Writer;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncIndex(pub(crate) i32);

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    set: ColumnarInterner<FuncCols>,
}

#[derive(Debug, Clone, Default)]
struct FuncCols {
    name: Vec<StringHandle>,
    source: Vec<Option<SourceIndex>>,
    start_line: Vec<Option<u32>>,
    start_column: Vec<Option<u32>>,
    resource: Vec<Option<ResourceIndex>>,
    flags: Vec<FrameFlags>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncKey {
    pub name: StringHandle,
    pub source: Option<SourceIndex>,
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    pub resource: Option<ResourceIndex>,
    pub flags: FrameFlags,
}

impl ColumnarStore for FuncCols {
    type Row = FuncKey;

    fn len(&self) -> usize {
        self.name.len()
    }

    fn hash_row<H: BuildHasher>(row: &FuncKey, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.name.hash(&mut h);
        row.source.hash(&mut h);
        row.start_line.hash(&mut h);
        row.start_column.hash(&mut h);
        row.resource.hash(&mut h);
        row.flags.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.name[i].hash(&mut h);
        self.source[i].hash(&mut h);
        self.start_line[i].hash(&mut h);
        self.start_column[i].hash(&mut h);
        self.resource[i].hash(&mut h);
        self.flags[i].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, i: usize, row: &FuncKey) -> bool {
        self.name[i] == row.name
            && self.source[i] == row.source
            && self.start_line[i] == row.start_line
            && self.start_column[i] == row.start_column
            && self.resource[i] == row.resource
            && self.flags[i] == row.flags
    }

    fn push(&mut self, row: FuncKey) {
        self.name.push(row.name);
        self.source.push(row.source);
        self.start_line.push(row.start_line);
        self.start_column.push(row.start_column);
        self.resource.push(row.resource);
        self.flags.push(row.flags);
    }
}

// The bits of the func table's `flags` column, as defined by the processed
// profile format (added in version 75). Each `HAS_*` bit determines whether
// the value in the corresponding column is meaningful.
// The format also has a `HasOriginalLocation` bit at `1 << 6`, but this crate
// does not emit original locations yet - those are used by source maps which
// our API doesn't support yet.
const FLAG_IS_JS: u8 = 1 << 0;
const FLAG_IS_RELEVANT_FOR_JS: u8 = 1 << 1;
const FLAG_HAS_RESOURCE: u8 = 1 << 2;
const FLAG_HAS_SOURCE: u8 = 1 << 3;
const FLAG_HAS_LINE: u8 = 1 << 4;
const FLAG_HAS_COLUMN: u8 = 1 << 5;

impl FuncCols {
    fn flags_at(&self, i: usize) -> u8 {
        let mut flags = 0;
        if self.flags[i].contains(FrameFlags::IS_JS) {
            flags |= FLAG_IS_JS;
        }
        if self.flags[i].contains(FrameFlags::IS_RELEVANT_FOR_JS) {
            flags |= FLAG_IS_RELEVANT_FOR_JS;
        }
        if self.resource[i].is_some() {
            flags |= FLAG_HAS_RESOURCE;
        }
        if self.source[i].is_some() {
            flags |= FLAG_HAS_SOURCE;
        }
        if self.start_line[i].is_some() {
            flags |= FLAG_HAS_LINE;
        }
        if self.start_column[i].is_some() {
            flags |= FLAG_HAS_COLUMN;
        }
        flags
    }
}

impl FuncTable {
    pub fn index_for_func(&mut self, func_key: FuncKey) -> FuncIndex {
        FuncIndex(self.set.insert(func_key) as i32)
    }

    pub(crate) fn write_json<'p, W: Write>(
        &'p self,
        w: &mut Writer<'_, 'p, W>,
    ) -> std::io::Result<()> {
        let cols = self.set.store();
        let len = self.set.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            // All columns can be typed arrays as of format version 75. The
            // columns which are gated by a flag bit store 0 in the rows where
            // the flag is unset.
            w.name("flags")?;
            w.typed_array_from_iter(len, (0..len).map(|i| cols.flags_at(i)))?;
            w.name("name")?;
            w.typed_array_from_iter(len, cols.name.iter().map(|n| n.as_u32() as i32))?;
            w.name("resource")?;
            w.typed_array_from_iter(
                len,
                cols.resource.iter().map(|r| match r {
                    Some(r) => r.as_i32(),
                    None => 0,
                }),
            )?;
            w.name("source")?;
            w.typed_array_from_iter(
                len,
                cols.source.iter().map(|s| match s {
                    Some(s) => s.as_i32(),
                    None => 0,
                }),
            )?;
            w.name("lineNumber")?;
            w.typed_array_from_iter(len, cols.start_line.iter().map(|l| l.unwrap_or(0) as i32))?;
            w.name("columnNumber")?;
            w.typed_array_from_iter(len, cols.start_column.iter().map(|c| c.unwrap_or(0) as i32))?;
            // We never have original locations, so no func has HasOriginalLocation.
            w.name("originalLocation")?;
            w.typed_array_from_iter(len, std::iter::repeat(0i32).take(len))
        })
    }
}

#[cfg(test)]
mod tests {
    use json_slabs::{ParsedFile, SlabType};

    use super::{FLAG_HAS_COLUMN, FLAG_HAS_LINE, FLAG_HAS_RESOURCE, FLAG_HAS_SOURCE, FLAG_IS_JS};
    use crate::jslb_test_support::{column_slab, object_at_path};
    use crate::{
        CategoryHandle, FrameAddress, FrameFlags, LibraryInfo, Profile, ProfileFormat,
        ReferenceTimestamp, SamplingInterval, SourceLocation,
    };

    /// The `funcTable` columns have to be typed arrays of the exact types the
    /// format specifies (processed format version 75), and the flag bits have
    /// to say which of the optional columns are meaningful.
    #[test]
    fn func_table_is_typed_arrays_in_jslb() {
        let mut profile = Profile::new(
            "test",
            ReferenceTimestamp::from_millis_since_unix_epoch(0.0),
            SamplingInterval::from_millis(1),
        );
        let lib = profile.add_lib(LibraryInfo {
            name: "libfoo.so".into(),
            debug_name: "libfoo.so".into(),
            path: "/usr/lib/libfoo.so".into(),
            debug_path: "/usr/lib/libfoo.so".into(),
            debug_id: debugid::DebugId::nil(),
            code_id: None,
            arch: None,
        });

        // A plain label frame: none of the optional columns are meaningful.
        let plain_name = profile.handle_for_string("plain label");
        profile.handle_for_frame_with_label(plain_name, CategoryHandle::OTHER, FrameFlags::empty());

        // A JS frame with a known function start location: IsJS + source + line
        // + column.
        let js_name = profile.handle_for_string("jsFunction");
        let file_path = profile.handle_for_string("https://example.com/script.js");
        profile.handle_for_frame_with_label_and_source_location(
            js_name,
            SourceLocation {
                file_path: Some(file_path),
                line: Some(20),
                col: Some(7),
                function_start_line: Some(10),
                function_start_col: Some(5),
            },
            CategoryHandle::OTHER,
            FrameFlags::IS_JS,
        );

        // A native frame: it has a resource (the library it came from).
        profile.handle_for_frame_with_address(
            FrameAddress::RelativeAddressFromInstructionPointer(lib, 0x1234),
            CategoryHandle::OTHER,
            FrameFlags::empty(),
        );

        let bytes = profile.to_vec(ProfileFormat::JsonSlabs);
        let file = ParsedFile::parse(&bytes).unwrap();
        let func_table = object_at_path(&file, &["shared", "funcTable"]);
        assert_eq!(func_table["length"], 3);

        let placeholder = |column: &str| column_slab(&func_table, column);
        let slab_type = |column: &str| file.slab_at(placeholder(column)).unwrap().slab_type;
        assert_eq!(slab_type("flags"), SlabType::Uint8);
        assert_eq!(slab_type("name"), SlabType::Int32);
        assert_eq!(slab_type("resource"), SlabType::Int32);
        assert_eq!(slab_type("source"), SlabType::Int32);
        assert_eq!(slab_type("lineNumber"), SlabType::Int32);
        assert_eq!(slab_type("columnNumber"), SlabType::Int32);
        assert_eq!(slab_type("originalLocation"), SlabType::Int32);

        assert_eq!(
            file.read::<u8>(placeholder("flags")).unwrap(),
            [
                0,
                FLAG_IS_JS | FLAG_HAS_SOURCE | FLAG_HAS_LINE | FLAG_HAS_COLUMN,
                FLAG_HAS_RESOURCE,
            ]
        );
        // The unsymbolicated native frame is named after its address.
        let native_name = profile.handle_for_string("0x1234");
        assert_eq!(
            file.read::<i32>(placeholder("name")).unwrap(),
            [
                plain_name.as_u32() as i32,
                js_name.as_u32() as i32,
                native_name.as_u32() as i32
            ]
        );
        // The native func is the only one with a resource, and it's the first
        // row of the resource table.
        assert_eq!(
            file.read::<i32>(placeholder("resource")).unwrap(),
            [0, 0, 0]
        );
        // Likewise for the source: the JS func's source is the first row of the
        // source table.
        assert_eq!(file.read::<i32>(placeholder("source")).unwrap(), [0, 0, 0]);
        // The line and column are the *function start* line and column, not the
        // line and column of the frame.
        assert_eq!(
            file.read::<i32>(placeholder("lineNumber")).unwrap(),
            [0, 10, 0]
        );
        assert_eq!(
            file.read::<i32>(placeholder("columnNumber")).unwrap(),
            [0, 5, 0]
        );
        assert_eq!(
            file.read::<i32>(placeholder("originalLocation")).unwrap(),
            [0, 0, 0]
        );
    }
}
