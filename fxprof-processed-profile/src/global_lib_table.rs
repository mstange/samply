use std::collections::BTreeSet;
use std::sync::Arc;

use serde::ser::{Serialize, Serializer};

use crate::fast_hash_map::{FastHashMap, FastIndexSet};
use crate::string_table::ProfileStringTable;
use crate::{LibraryInfo, StringHandle, SymbolTable};

#[derive(Debug)]
pub struct GlobalLibTable {
    /// All libraries added via `Profile::handle_for_lib`. May or may not be used.
    /// Indexed by `LibraryHandle.0`.
    all_libs: FastIndexSet<LibraryInfo>, // append-only for stable LibraryHandles
    /// Any symbol tables for libraries in all_libs
    symbol_tables: FastHashMap<LibraryHandle, Arc<SymbolTable>>,
    /// Indexed by `GlobalLibIndex.0`.
    used_libs: Vec<LibraryHandle>, // append-only for stable GlobalLibIndexes
    used_lib_map: FastHashMap<LibraryHandle, GlobalLibIndex>,
}

#[derive(Debug)]
pub struct UsedLibraryAddressesCollector {
    /// We keep track of RVA addresses that exist in frames that are assigned to this
    /// library, so that we can potentially provide symbolication info ahead of time.
    /// This is here instead of in `LibraryInfo` because we don't want to serialize it,
    /// and because it's currently a hack.
    /// Indexed by GlobalLibIndex.0, i.e. a parallel array to `used_libs`.
    used_libs_seen_rvas: Vec<BTreeSet<u32>>,
}

impl GlobalLibTable {
    pub fn new() -> Self {
        Self {
            all_libs: FastIndexSet::default(),
            symbol_tables: FastHashMap::default(),
            used_libs: Vec::new(),
            used_lib_map: FastHashMap::default(),
        }
    }

    pub fn handle_for_lib(&mut self, lib: LibraryInfo) -> LibraryHandle {
        LibraryHandle(self.all_libs.insert_full(lib).0)
    }

    pub fn get_lib(&self, handle: LibraryHandle) -> &LibraryInfo {
        self.all_libs.get_index(handle.0).unwrap()
    }

    pub fn set_lib_symbol_table(&mut self, library: LibraryHandle, symbol_table: Arc<SymbolTable>) {
        self.symbol_tables.insert(library, symbol_table);
    }

    pub fn index_for_used_lib(
        &mut self,
        lib_handle: LibraryHandle,
        string_table: &mut ProfileStringTable,
    ) -> GlobalLibIndex {
        let used_libs = &mut self.used_libs;
        *self.used_lib_map.entry(lib_handle).or_insert_with(|| {
            let lib = self.all_libs.get_index(lib_handle.0).unwrap();
            let name_index = string_table.index_for_string(&lib.name);
            let index = GlobalLibIndex(used_libs.len(), name_index);
            used_libs.push(lib_handle);
            index
        })
    }

    pub fn library_handle_for_used_lib_index(&self, used_lib_index: usize) -> LibraryHandle {
        self.used_libs[used_lib_index]
    }

    pub fn used_lib_index(&self, lib_handle: LibraryHandle) -> Option<GlobalLibIndex> {
        self.used_lib_map.get(&lib_handle).cloned()
    }

    pub fn get_lib_symbol_table(&self, index: GlobalLibIndex) -> Option<&SymbolTable> {
        let handle = self.used_libs.get(index.0)?;
        self.symbol_tables.get(handle).map(|v| &**v)
    }

    pub fn address_collector(&self) -> UsedLibraryAddressesCollector {
        UsedLibraryAddressesCollector {
            used_libs_seen_rvas: vec![BTreeSet::new(); self.used_libs.len()],
        }
    }
}

impl UsedLibraryAddressesCollector {
    pub fn add_lib_used_rva(&mut self, index: GlobalLibIndex, address: u32) {
        self.used_libs_seen_rvas[index.0].insert(address);
    }

    pub fn finish(self, global_lib_table: &GlobalLibTable) -> Vec<(LibraryHandle, BTreeSet<u32>)> {
        self.used_libs_seen_rvas
            .into_iter()
            .enumerate()
            .map(|(used_lib_index, rvas)| {
                let lib_handle = global_lib_table.library_handle_for_used_lib_index(used_lib_index);
                (lib_handle, rvas)
            })
            .collect()
    }
}

impl Serialize for GlobalLibTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.used_libs.iter().map(|handle| &self.all_libs[handle.0]))
    }
}

/// An index for a *used* library, i.e. a library for which there exists at
/// least one frame in any process's frame table which refers to this lib.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct GlobalLibIndex(usize, StringHandle);

impl Serialize for GlobalLibIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0 as u32)
    }
}

impl GlobalLibIndex {
    pub fn name_string_index(&self) -> StringHandle {
        self.1
    }
}

/// The handle for a library, obtained from [`Profile::add_lib`](crate::Profile::add_lib).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct LibraryHandle(usize);
