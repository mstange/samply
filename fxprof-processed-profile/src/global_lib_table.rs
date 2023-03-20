use serde::ser::{Serialize, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::LibraryInfo;

#[derive(Debug)]
pub struct GlobalLibTable {
    /// All libraries added via `Profile::add_lib`. May or may not be used.
    /// Indexed by `LibraryHandle.0`.
    all_libs: Vec<LibraryInfo>, // append-only for stable LibraryHandles
    /// Indexed by `GlobalLibIndex.0`.
    used_libs: Vec<LibraryHandle>, // append-only for stable GlobalLibIndexes
    lib_map: FastHashMap<LibraryInfo, LibraryHandle>,
    used_lib_map: FastHashMap<LibraryHandle, GlobalLibIndex>,
}

impl GlobalLibTable {
    pub fn new() -> Self {
        Self {
            all_libs: Vec::new(),
            used_libs: Vec::new(),
            lib_map: FastHashMap::default(),
            used_lib_map: FastHashMap::default(),
        }
    }

    pub fn handle_for_lib(&mut self, lib: LibraryInfo) -> LibraryHandle {
        let all_libs = &mut self.all_libs;
        *self.lib_map.entry(lib.clone()).or_insert_with(|| {
            let handle = LibraryHandle(all_libs.len());
            all_libs.push(lib);
            handle
        })
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
pub struct LibraryHandle(usize);
