use std::hash::{BuildHasher, Hash, Hasher};
use std::io::Write;

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::fast_hash_map::FastHashSet;
use crate::global_lib_table::GlobalLibIndex;
use crate::string_table::StringHandle;
use crate::writer::Writer;

/// Represents a symbol from the symbol table of a library. Obtained from [`Profile::handle_for_native_symbol`](crate::Profile::handle_for_native_symbol).
///
/// Used on native stack frames, i.e. on frames with a code address. The native
/// symbol is used for the assembly view in the front-end. Every native symbol
/// represents a sequence of assembly instructions.
///
/// ## Examples of native symbols
///
/// - A "standalone copy" of a compiled C++ function, i.e. something that can be called
///   with a `call` instruction.
/// - A JIT-compiled JavaScript function. Every new compilation would be a separate
///   native symbol, because it's a separate chunk of native code / assembly instructions.
///
/// ## Interactions with inlining
///
/// When function A calls function B, the compiler may choose to inline this call into the
/// generated code for A. In that case, B ends up contributed some instructions to A's
/// generated code.
/// These instructions have an "inline stack": A -> B. If such an instruction is sampled
/// by the profiler, this is represented as follows:
///
/// - One native symbol is created, for A. There is no native symbol for B because there
///   is no standalone copy of native code for B.
/// - Two frames are created for this instruction address, and they both share the same
///   frame address and the same native symbol.
/// - The two frames have different function names, and potentially different file paths
///   and line numbers, if this information is known.
/// - The frame for A has inline depth 0 and the frame for B has inline depth 1.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeSymbolHandle(pub(crate) NativeSymbolIndex);

/// The native symbols that are used by frames in a thread's `FrameTable`.
/// They can be from different libraries. Only used symbols are included.
#[derive(Debug, Clone, Default)]
pub struct NativeSymbols {
    set: ColumnarInterner<NativeSymbolCols>,
}

#[derive(Debug, Clone, Default)]
struct NativeSymbolCols {
    addresses: Vec<u32>,
    function_sizes: Vec<i32>,
    lib_indexes: Vec<GlobalLibIndex>,
    names: Vec<StringHandle>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeSymbolKey {
    pub lib_index: GlobalLibIndex,
    pub address: u32,
    /// The size of the function's machine code, in bytes, or
    /// [`FUNCTION_SIZE_UNKNOWN`] if the size isn't known.
    pub function_size: i32,
    pub name: StringHandle,
}

/// The value the processed profile format uses in the native symbol table's
/// `functionSize` column when the size of the function isn't known.
/// (Before format version 74, this was `null`.)
const FUNCTION_SIZE_UNKNOWN: i32 = -1;

impl ColumnarStore for NativeSymbolCols {
    type Row = NativeSymbolKey;

    fn len(&self) -> usize {
        self.addresses.len()
    }

    fn hash_row<H: BuildHasher>(row: &NativeSymbolKey, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.lib_index.hash(&mut h);
        row.address.hash(&mut h);
        row.function_size.hash(&mut h);
        row.name.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.lib_indexes[i].hash(&mut h);
        self.addresses[i].hash(&mut h);
        self.function_sizes[i].hash(&mut h);
        self.names[i].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, i: usize, row: &NativeSymbolKey) -> bool {
        self.lib_indexes[i] == row.lib_index
            && self.addresses[i] == row.address
            && self.function_sizes[i] == row.function_size
            && self.names[i] == row.name
    }

    fn push(&mut self, row: NativeSymbolKey) {
        self.lib_indexes.push(row.lib_index);
        self.addresses.push(row.address);
        self.function_sizes.push(row.function_size);
        self.names.push(row.name);
    }
}

pub struct NativeSymbolIndexTranslator(Vec<u32>);

impl NativeSymbolIndexTranslator {
    pub fn map(&self, index: NativeSymbolIndex) -> NativeSymbolIndex {
        NativeSymbolIndex(self.0[index.0 as usize])
    }
}

impl NativeSymbols {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn symbol_index_for_symbol(
        &mut self,
        lib_index: GlobalLibIndex,
        symbol_address: u32,
        symbol_size: Option<u32>,
        symbol_name_string_index: StringHandle,
    ) -> NativeSymbolIndex {
        let function_size = match symbol_size {
            Some(size) => size as i32,
            None => FUNCTION_SIZE_UNKNOWN,
        };
        NativeSymbolIndex(self.set.insert(NativeSymbolKey {
            lib_index,
            address: symbol_address,
            function_size,
            name: symbol_name_string_index,
        }))
    }

    pub fn new_table_with_symbols_from_libs_removed(
        self,
        libs: &FastHashSet<GlobalLibIndex>,
    ) -> (NativeSymbols, NativeSymbolIndexTranslator) {
        let cols = self.set.into_store();
        let old_len = cols.addresses.len();
        let mut old_index_to_new_index = Vec::with_capacity(old_len);
        let mut new_table = NativeSymbols::new();
        for i in 0..old_len {
            let lib_index = cols.lib_indexes[i];
            if libs.contains(&lib_index) {
                old_index_to_new_index.push(0);
            } else {
                let new_idx = new_table.set.insert(NativeSymbolKey {
                    lib_index,
                    address: cols.addresses[i],
                    function_size: cols.function_sizes[i],
                    name: cols.names[i],
                });
                old_index_to_new_index.push(new_idx);
            }
        }
        (
            new_table,
            NativeSymbolIndexTranslator(old_index_to_new_index),
        )
    }

    pub fn get_native_symbol_name(&self, native_symbol_index: NativeSymbolIndex) -> StringHandle {
        self.set.store().names[native_symbol_index.0 as usize]
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
            // All four columns can be typed arrays as of format version 74.
            w.name("address")?;
            w.typed_array(&cols.addresses)?;
            w.name("functionSize")?;
            w.typed_array(&cols.function_sizes)?;
            w.name("libIndex")?;
            w.typed_array_from_iter(len, cols.lib_indexes.iter().map(|li| li.as_i32()))?;
            w.name("name")?;
            w.typed_array_from_iter(len, cols.names.iter().map(|n| n.as_u32() as i32))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeSymbolIndex(u32);

impl NativeSymbolIndex {
    pub(crate) fn as_i32(self) -> i32 {
        self.0 as i32
    }
}

#[cfg(test)]
mod tests {
    use json_slabs::{ParsedFile, SlabPlaceholder, SlabType, SLAB_REF_KEY};

    use crate::{
        LibraryInfo, Profile, ProfileFormat, ReferenceTimestamp, SamplingInterval, Timestamp,
    };

    /// The `nativeSymbols` columns have to be typed arrays of the exact types
    /// the format specifies (processed format version 74), and the "size
    /// unknown" sentinel has to be `-1`.
    #[test]
    fn native_symbols_are_typed_arrays_in_jslb() {
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
        let process = profile.add_process("test", 123, Timestamp::from_millis_since_reference(0.0));
        let _thread = profile.add_thread(
            process,
            12345,
            Timestamp::from_millis_since_reference(0.0),
            true,
        );
        let name_1 = profile.handle_for_string("known_size");
        let name_2 = profile.handle_for_string("unknown_size");
        profile.handle_for_native_symbol(lib, 0x1000, Some(0x20), name_1);
        profile.handle_for_native_symbol(lib, 0x2000, None, name_2);

        let bytes = profile.to_vec(ProfileFormat::JsonSlabs);
        let file = ParsedFile::parse(&bytes).unwrap();
        let root: serde_json::Value = serde_json::from_slice(file.root_json_bytes()).unwrap();
        let native_symbols = &root["shared"]["nativeSymbols"];
        assert_eq!(native_symbols["length"], 2);

        let placeholder = |column: &str| -> SlabPlaceholder {
            let index = native_symbols[column][SLAB_REF_KEY]
                .as_u64()
                .unwrap_or_else(|| panic!("{column} should be a slab reference"));
            SlabPlaceholder(index as usize)
        };
        let slab_type = |column: &str| file.slab_at(placeholder(column)).unwrap().slab_type;
        assert_eq!(slab_type("libIndex"), SlabType::Int32);
        assert_eq!(slab_type("address"), SlabType::Uint32);
        assert_eq!(slab_type("name"), SlabType::Int32);
        assert_eq!(slab_type("functionSize"), SlabType::Int32);

        assert_eq!(
            file.read::<u32>(placeholder("address")).unwrap(),
            [0x1000, 0x2000]
        );
        assert_eq!(
            file.read::<i32>(placeholder("functionSize")).unwrap(),
            [0x20, -1]
        );
        assert_eq!(
            file.read::<i32>(placeholder("name")).unwrap(),
            [name_1.as_u32() as i32, name_2.as_u32() as i32]
        );
    }
}
