use std::hash::{BuildHasher, Hash, Hasher};

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::fast_hash_map::FastHashSet;
use crate::global_lib_table::GlobalLibIndex;
use crate::string_table::StringHandle;

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
    function_sizes: Vec<Option<u32>>,
    lib_indexes: Vec<GlobalLibIndex>,
    names: Vec<StringHandle>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeSymbolKey {
    pub lib_index: GlobalLibIndex,
    pub address: u32,
    pub function_size: Option<u32>,
    pub name: StringHandle,
}

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
        NativeSymbolIndex(self.set.insert(NativeSymbolKey {
            lib_index,
            address: symbol_address,
            function_size: symbol_size,
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
                let new_idx = new_table.symbol_index_for_symbol(
                    lib_index,
                    cols.addresses[i],
                    cols.function_sizes[i],
                    cols.names[i],
                );
                old_index_to_new_index.push(new_idx.0);
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
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct NativeSymbolIndex(u32);

impl Serialize for NativeSymbolIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl Serialize for NativeSymbols {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let cols = self.set.store();
        let len = self.set.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("address", &cols.addresses)?;
        map.serialize_entry("functionSize", &cols.function_sizes)?;
        map.serialize_entry("libIndex", &cols.lib_indexes)?;
        map.serialize_entry("name", &cols.names)?;
        map.end()
    }
}
