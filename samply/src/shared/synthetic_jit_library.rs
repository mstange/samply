use std::sync::Arc;

use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryPairHandle, LibraryHandle, LibraryInfo, Profile, Symbol, SymbolTable,
};

use super::types::FastHashMap;

#[derive(Debug)]
pub struct SyntheticJitLibrary {
    lib_handle: LibraryHandle,
    default_category: CategoryPairHandle,
    next_relative_address: u32,
    symbols: Vec<Symbol>,
    recycler: Option<FastHashMap<(String, u32), u32>>,
}

impl SyntheticJitLibrary {
    pub fn new(
        name: String,
        default_category: CategoryPairHandle,
        profile: &mut Profile,
        allow_recycling: bool,
    ) -> Self {
        let lib_handle = profile.add_lib(LibraryInfo {
            name: name.clone(),
            debug_name: name.clone(),
            path: name.clone(),
            debug_path: name,
            debug_id: DebugId::nil(),
            code_id: None,
            arch: None,
            symbol_table: None,
        });
        let recycler = if allow_recycling {
            Some(FastHashMap::default())
        } else {
            None
        };
        Self {
            lib_handle,
            default_category,
            next_relative_address: 0,
            symbols: Vec::new(),
            recycler,
        }
    }

    /// Returns the relative address of the added function.
    pub fn add_function(&mut self, name: String, size: u32) -> u32 {
        if let Some(recycler) = self.recycler.as_mut() {
            let key = (name, size);
            if let Some(relative_address) = recycler.get(&key) {
                return *relative_address;
            }
            let relative_address = self.next_relative_address;
            self.next_relative_address += size;
            self.symbols.push(Symbol {
                address: relative_address,
                size: Some(size),
                name: key.0.clone(),
            });
            recycler.insert(key, relative_address);
            relative_address
        } else {
            let relative_address = self.next_relative_address;
            self.next_relative_address += size;
            self.symbols.push(Symbol {
                address: relative_address,
                size: Some(size),
                name,
            });
            relative_address
        }
    }

    pub fn lib_handle(&self) -> LibraryHandle {
        self.lib_handle
    }

    pub fn default_category(&self) -> CategoryPairHandle {
        self.default_category
    }

    pub fn finish_and_set_symbol_table(self, profile: &mut Profile) {
        let symbol_table = Arc::new(SymbolTable::new(self.symbols));
        profile.set_lib_symbol_table(self.lib_handle, symbol_table);
    }
}
