use serde::ser::{Serialize, Serializer};

use crate::string_table::{GlobalStringIndex, GlobalStringTable, StringIndex};
use crate::{fast_hash_map::FastHashMap, string_table::StringTable};

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ThreadInternalStringIndex(pub StringIndex);

impl Serialize for ThreadInternalStringIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadStringTable {
    table: StringTable,
    global_to_local_string: FastHashMap<GlobalStringIndex, ThreadInternalStringIndex>,
}

impl ThreadStringTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_string(&mut self, s: &str) -> ThreadInternalStringIndex {
        ThreadInternalStringIndex(self.table.index_for_string(s))
    }

    pub fn index_for_global_string(
        &mut self,
        global_index: GlobalStringIndex,
        global_table: &GlobalStringTable,
    ) -> ThreadInternalStringIndex {
        let table = &mut self.table;
        *self
            .global_to_local_string
            .entry(global_index)
            .or_insert_with(|| {
                let s = global_table.get_string(global_index).unwrap();
                ThreadInternalStringIndex(table.index_for_string(s))
            })
    }
}

impl Serialize for ThreadStringTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.table.serialize(serializer)
    }
}
