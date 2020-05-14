use object::{Object, SymbolKind};
use std::collections::HashMap;
use std::ops::Deref;

pub trait SymbolTableResult {
    fn from_map<S>(map: HashMap<u32, S>) -> Self
    where
        S: Deref<Target = str>;
}
#[repr(C)]
pub struct CompactSymbolTable {
    pub addr: Vec<u32>,
    pub index: Vec<u32>,
    pub buffer: Vec<u8>,
}

impl CompactSymbolTable {
    pub fn new() -> Self {
        Self {
            addr: Vec::new(),
            index: Vec::new(),
            buffer: Vec::new(),
        }
    }

    fn add_name(&mut self, name: &str) {
        self.buffer.extend_from_slice(name.as_bytes());
    }
}

impl SymbolTableResult for CompactSymbolTable {
    fn from_map<T: Deref<Target = str>>(map: HashMap<u32, T>) -> Self {
        let mut table = Self::new();
        let mut entries: Vec<_> = map.into_iter().collect();
        entries.sort_by_key(|&(addr, _)| addr);
        for (addr, name) in entries {
            table.addr.push(addr);
            table.index.push(table.buffer.len() as u32);
            table.add_name(&name);
        }
        table.index.push(table.buffer.len() as u32);
        table
    }
}

pub fn object_to_map<'a, 'b, T>(object_file: &'b T) -> HashMap<u32, &'a str>
where
    T: Object<'a, 'b>,
{
    object_file
        .dynamic_symbols()
        .chain(object_file.symbols())
        .filter(|(_, symbol)| symbol.kind() == SymbolKind::Text)
        .filter_map(|(_, symbol)| symbol.name().map(|name| (symbol.address() as u32, name)))
        .collect()
}
