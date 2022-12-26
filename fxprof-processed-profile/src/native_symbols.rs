use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::{
    fast_hash_map::FastHashMap,
    global_lib_table::GlobalLibIndex,
    library_info::Symbol,
    thread_string_table::{ThreadInternalStringIndex, ThreadStringTable},
};

/// The native symbols that are used by frames in a thread's `FrameTable`.
/// They can be from different libraries. Only used symbols are included.
#[derive(Debug, Clone, Default)]
pub struct NativeSymbols {
    addresses: Vec<u32>,
    function_sizes: Vec<Option<u32>>,
    lib_indexes: Vec<GlobalLibIndex>,
    names: Vec<ThreadInternalStringIndex>,

    lib_and_symbol_address_to_symbol_index: FastHashMap<(GlobalLibIndex, u32), usize>,
}

impl NativeSymbols {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn symbol_index_and_string_index_for_symbol(
        &mut self,
        lib_index: GlobalLibIndex,
        symbol: &Symbol,
        string_table: &mut ThreadStringTable,
    ) -> (NativeSymbolIndex, ThreadInternalStringIndex) {
        let addresses = &mut self.addresses;
        let function_sizes = &mut self.function_sizes;
        let lib_indexes = &mut self.lib_indexes;
        let names = &mut self.names;
        let symbol_index = *self
            .lib_and_symbol_address_to_symbol_index
            .entry((lib_index, symbol.address))
            .or_insert_with(|| {
                let native_symbol_index = addresses.len();
                addresses.push(symbol.address);
                function_sizes.push(symbol.size);
                lib_indexes.push(lib_index);
                names.push(string_table.index_for_string(&symbol.name));
                native_symbol_index
            });
        let name_string_index = names[symbol_index];
        (NativeSymbolIndex(symbol_index as u32), name_string_index)
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
