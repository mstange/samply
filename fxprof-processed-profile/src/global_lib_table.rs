use std::collections::BTreeSet;
use std::sync::Arc;

use serde::ser::{Serialize, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::{LibraryInfo, SymbolTable};

#[derive(Debug)]
pub struct GlobalLibTable {
    /// All libraries added via `Profile::add_lib`. May or may not be used.
    /// Indexed by `LibraryHandle.0`.
    all_libs: Vec<LibraryInfo>, // append-only for stable LibraryHandles
    /// Indexed by `GlobalLibIndex.0`.
    used_libs: Vec<LibraryHandle>, // append-only for stable GlobalLibIndexes
    lib_map: FastHashMap<LibraryInfo, LibraryHandle>,
    used_lib_map: FastHashMap<LibraryHandle, GlobalLibIndex>,
    /// We keep track of RVA addresses that exist in frames that are assigned to this
    /// library, so that we can potentially provide symbolication info ahead of time.
    /// This is here instead of in `LibraryInfo` because we don't want to serialize it,
    /// and because it's currently a hack.
    all_libs_seen_rvas: Vec<Option<BTreeSet<u32>>>,
}

impl GlobalLibTable {
    pub fn new() -> Self {
        Self {
            all_libs: Vec::new(),
            used_libs: Vec::new(),
            lib_map: FastHashMap::default(),
            used_lib_map: FastHashMap::default(),
            all_libs_seen_rvas: Vec::new(),
        }
    }

    pub fn handle_for_lib(&mut self, lib: LibraryInfo) -> LibraryHandle {
        let all_libs = &mut self.all_libs;
        *self.lib_map.entry(lib.clone()).or_insert_with(|| {
            let handle = LibraryHandle(all_libs.len());
            all_libs.push(lib);
            self.all_libs_seen_rvas.push(None);
            handle
        })
    }

    pub fn set_lib_symbol_table(&mut self, library: LibraryHandle, symbol_table: Arc<SymbolTable>) {
        self.all_libs[library.0].symbol_table = Some(symbol_table);
    }

    pub fn index_for_used_lib(&mut self, lib_handle: LibraryHandle) -> GlobalLibIndex {
        let used_libs = &mut self.used_libs;
        *self.used_lib_map.entry(lib_handle).or_insert_with(|| {
            let index = GlobalLibIndex(used_libs.len());
            used_libs.push(lib_handle);
            index
        })
    }

    pub fn get_lib(&self, index: GlobalLibIndex) -> Option<&LibraryInfo> {
        let handle = self.used_libs.get(index.0)?;
        self.all_libs.get(handle.0)
    }

    pub fn add_lib_used_rva(&mut self, index: GlobalLibIndex, address: u32) {
        let handle = self.used_libs.get(index.0).unwrap();
        let lib_seen_rvas = self.all_libs_seen_rvas[handle.0].get_or_insert_with(BTreeSet::new);
        lib_seen_rvas.insert(address);
    }

    pub fn lib_used_rva_iter(&self) -> UsedLibraryAddressesIterator {
        UsedLibraryAddressesIterator {
            next_used_lib_index: 0,
            global_lib_table: self,
        }
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
pub struct GlobalLibIndex(usize);

impl Serialize for GlobalLibIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0 as u32)
    }
}

/// The handle for a library, obtained from [`Profile::add_lib`](crate::Profile::add_lib).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct LibraryHandle(pub(crate) usize);

pub struct UsedLibraryAddressesIterator<'a> {
    next_used_lib_index: usize,
    global_lib_table: &'a GlobalLibTable,
}

impl<'a> Iterator for UsedLibraryAddressesIterator<'a> {
    type Item = (&'a LibraryInfo, Option<&'a BTreeSet<u32>>);

    fn next(&mut self) -> Option<Self::Item> {
        self.global_lib_table
            .used_libs
            .get(self.next_used_lib_index)
            .map(|lib| {
                self.next_used_lib_index += 1;
                (
                    &self.global_lib_table.all_libs[lib.0],
                    self.global_lib_table.all_libs_seen_rvas[lib.0].as_ref(),
                )
            })
    }
}
