use std::io::Write;

use crate::fast_hash_map::FastHashMap;
use crate::writer::Writer;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StringIndex(u32);

impl StringIndex {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

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

    pub fn get_string(&self, index: StringIndex) -> &str {
        &self.strings[index.0 as usize]
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }
}

/// The handle for an interned string in the profile's string table. Created
/// with [`Profile::handle_for_string`](crate::Profile::handle_for_string).
///
/// String handles are how the profile keeps its JSON small: every string that
/// appears in a marker, frame, or thread name is interned once in a per-profile
/// string table, and all references to it are integer handles into that table.
/// Calling `handle_for_string` with the same string twice always returns the
/// same handle.
///
/// The handle is specific to the [`Profile`](crate::Profile) instance it was
/// created from. Using it with a different `Profile` will produce nonsense
/// strings or panics. Storing and reusing the handle avoids repeated lookups.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StringHandle(pub(crate) StringIndex);

impl StringHandle {
    pub(crate) fn as_u32(self) -> u32 {
        self.0.as_u32()
    }

    pub(crate) fn write_json<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.number_value(self.as_u32())
    }

    pub(crate) fn write_optional<W: Write>(
        this: Option<StringHandle>,
        w: &mut Writer<W>,
    ) -> std::io::Result<()> {
        match this {
            Some(h) => w.number_value(h.as_u32()),
            None => w.null_value(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProfileStringTable {
    table: StringTable,
    hex_address_strings: FastHashMap<u64, StringHandle>,
}

impl ProfileStringTable {
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
            let mut buf = [0u8; BUF_LEN]; // 18 is just enough to fit u64::MAX, i.e. "0xffffffffffffffff"
            let mut b = &mut buf[..];
            write!(b, "{a:#x}").unwrap();
            let len = BUF_LEN - b.len();
            let s = std::str::from_utf8(&buf[..len]).unwrap();
            StringHandle(self.table.index_for_string(s))
        })
    }

    pub fn get_string(&self, index: StringHandle) -> &str {
        self.table.get_string(index.0)
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.array(|w| {
            for s in self.table.strings() {
                w.string_value(s)?;
            }
            Ok(())
        })
    }
}
