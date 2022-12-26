use serde::ser::{Serialize, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::lib_info::Lib;

#[derive(Debug)]
pub struct GlobalLibTable {
    libs: Vec<Lib>, // append-only for stable GlobalLibIndexes
    lib_map: FastHashMap<Lib, GlobalLibIndex>,
}

impl GlobalLibTable {
    pub fn new() -> Self {
        Self {
            libs: Vec::new(),
            lib_map: FastHashMap::default(),
        }
    }

    pub fn index_for_lib(&mut self, lib: Lib) -> GlobalLibIndex {
        let libs = &mut self.libs;
        *self.lib_map.entry(lib.clone()).or_insert_with(|| {
            let index = GlobalLibIndex(libs.len());
            libs.push(lib);
            index
        })
    }

    pub fn get_lib(&self, index: GlobalLibIndex) -> Option<&Lib> {
        self.libs.get(index.0)
    }
}

impl Serialize for GlobalLibTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.libs.serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct GlobalLibIndex(usize);

impl Serialize for GlobalLibIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0 as u32)
    }
}
