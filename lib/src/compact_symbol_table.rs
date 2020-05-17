use super::shared::{AddressDebugInfo, SymbolicationResult, SymbolicationResultKind};
use std::collections::BTreeMap;
use std::ops::Deref;

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
    fn from_full_map<T: Deref<Target = str>>(map: BTreeMap<u32, T>, _addresses: &[u32]) -> Self {
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

    fn for_addresses(_addresses: &[u32]) -> Self {
        panic!("Should not be called")
    }

    fn result_kind() -> SymbolicationResultKind {
        SymbolicationResultKind::AllSymbols
    }

    fn add_address_symbol(&mut self, _address: u32, _symbol_address: u32, _symbol_name: &str) {
        panic!("Should not be called")
    }

    fn add_address_debug_info(&mut self, _address: u32, _info: AddressDebugInfo) {
        panic!("Should not be called")
    }

    fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {}
}
