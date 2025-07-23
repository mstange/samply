use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::fast_hash_map::{FastHashMap, FastHashSet};
use crate::global_lib_table::GlobalLibIndex;
use crate::library_info::Symbol;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::ThreadHandle;

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
pub struct NativeSymbolHandle(pub(crate) ThreadHandle, pub(crate) NativeSymbolIndex);

/// The native symbols that are used by frames in a thread's `FrameTable`.
/// They can be from different libraries. Only used symbols are included.
#[derive(Debug, Clone, Default)]
pub struct NativeSymbols {
    addresses: Vec<u32>,
    function_sizes: Vec<Option<u32>>,
    lib_indexes: Vec<GlobalLibIndex>,
    names: Vec<StringHandle>,

    lib_and_symbol_address_to_symbol_index: FastHashMap<(GlobalLibIndex, u32), usize>,
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
        let symbol_index = *self
            .lib_and_symbol_address_to_symbol_index
            .entry((lib_index, symbol_address))
            .or_insert_with(|| {
                let native_symbol_index = self.addresses.len();
                self.addresses.push(symbol_address);
                self.function_sizes.push(symbol_size);
                self.lib_indexes.push(lib_index);
                self.names.push(symbol_name_string_index);
                native_symbol_index
            });
        NativeSymbolIndex(symbol_index as u32)
    }

    pub fn symbol_index_and_string_index_for_symbol(
        &mut self,
        lib_index: GlobalLibIndex,
        symbol: &Symbol,
        string_table: &mut ProfileStringTable,
    ) -> (NativeSymbolIndex, StringHandle) {
        let name_string_index = string_table.index_for_string(&symbol.name);
        let symbol_index =
            self.symbol_index_for_symbol(lib_index, symbol.address, symbol.size, name_string_index);
        (symbol_index, name_string_index)
    }

    pub fn new_table_with_symbols_from_libs_removed(
        mut self,
        libs: &FastHashSet<GlobalLibIndex>,
    ) -> (NativeSymbols, NativeSymbolIndexTranslator) {
        let old_len = self.addresses.len();
        let mut old_index_to_new_index = Vec::with_capacity(old_len);
        let mut old_index = 0;
        let mut new_index = 0;
        while old_index < old_len {
            let lib_index = self.lib_indexes[old_index];
            if libs.contains(&lib_index) {
                // Remove.
                old_index_to_new_index.push(0);
                old_index += 1;
            } else {
                // Retain.
                old_index_to_new_index.push(new_index as u32);

                let address = self.addresses[old_index];
                self.addresses[new_index] = address;
                self.function_sizes[new_index] = self.function_sizes[old_index];
                self.lib_indexes[new_index] = lib_index;
                self.names[new_index] = self.names[old_index];
                self.lib_and_symbol_address_to_symbol_index
                    .insert((lib_index, address), new_index);

                old_index += 1;
                new_index += 1;
            }
        }
        (self, NativeSymbolIndexTranslator(old_index_to_new_index))
    }

    pub fn get_native_symbol_name(&self, native_symbol_index: NativeSymbolIndex) -> StringHandle {
        self.names[native_symbol_index.0 as usize]
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
        let len = self.names.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("address", &self.addresses)?;
        map.serialize_entry("functionSize", &self.function_sizes)?;
        map.serialize_entry("libIndex", &self.lib_indexes)?;
        map.serialize_entry("name", &self.names)?;
        map.end()
    }
}
