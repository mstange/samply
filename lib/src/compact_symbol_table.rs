use std::collections::HashMap;
use std::ops::Deref;
use super::shared::SymbolicationResult;

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

impl SymbolicationResult for CompactSymbolTable {
    fn from_map<T: Deref<Target = str>>(map: HashMap<u32, T>, _addresses: &[u32]) -> Self {
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
