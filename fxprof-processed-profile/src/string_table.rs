use std::ops::Deref;

use serde::ser::{Serialize, Serializer};

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
pub struct StringHandle(pub(crate) StringIndex);

#[derive(Debug, Clone, Default)]
pub struct GlobalStringTable {
    table: StringTable,
    hex_address_strings: FastHashMap<u64, StringHandle>,
}

impl GlobalStringTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_string(&mut self, s: &str) -> StringHandle {
        StringHandle(self.table.index_for_string(s))
    }

    // Fast path with separate cache for strings of the shape 0xabc123
    pub fn index_for_hex_address_string(&mut self, a: u64) -> StringHandle {
        *self.hex_address_strings.entry(a).or_insert_with(|| {
            // Build the string on the stack, to save a heap allocation.
            const BUF_LEN: usize = 18;
            let mut buf = [0u8; BUF_LEN]; // 18 is just enough to fix u64::MAX, i.e. "0xffffffffffffffff"
            use std::io::Write;
            let mut b = &mut buf[..];
            write!(b, "{a:#x}").unwrap();
            let len = BUF_LEN - b.len();
            let s = std::str::from_utf8(&buf[..len]).unwrap();
            StringHandle(self.table.index_for_string(s))
        })
    }

    pub fn get_string(&self, index: StringHandle) -> Option<&str> {
        self.table.get_string(index.0)
    }
}

impl Serialize for StringIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl Serialize for StringHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl Serialize for GlobalStringTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.table.serialize(serializer)
    }
}
