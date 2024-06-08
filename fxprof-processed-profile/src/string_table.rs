use std::ops::Deref;

use serde::{Serialize, Serializer};

use crate::fast_hash_map::FastHashMap;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StringIndex(u32);

#[derive(Debug, Clone, Default)]
pub struct StringTable {
    strings: Vec<String>,
    index: FastHashMap<String, StringIndex>,
}

impl StringTable {
    pub fn index_for_string(&mut self, s: &str) -> StringIndex {
        match self.index.get(s) {
            Some(string_index) => *string_index,
            None => {
                let string_index = StringIndex(self.strings.len() as u32);
                self.strings.push(s.to_string());
                self.index.insert(s.to_string(), string_index);
                string_index
            }
        }
    }

    pub fn get_string(&self, index: StringIndex) -> Option<&str> {
        self.strings.get(index.0 as usize).map(Deref::deref)
    }
}

impl Serialize for StringTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.strings.serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct GlobalStringIndex(pub(crate) StringIndex);

#[derive(Debug, Clone, Default)]
pub struct GlobalStringTable {
    table: StringTable,
}

impl GlobalStringTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_string(&mut self, s: &str) -> GlobalStringIndex {
        GlobalStringIndex(self.table.index_for_string(s))
    }

    pub fn get_string(&self, index: GlobalStringIndex) -> Option<&str> {
        self.table.get_string(index.0)
    }
}

impl Serialize for StringIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}
