use super::shared::{AddressDebugInfo, SymbolicationResult};
use std::ops::Deref;

/// A "compact" representation of a symbol table.
/// This is a legacy concept used by the Firefox profiler and kept for
/// compatibility purposes. It's called `SymbolTableAsTuple` in the profiler code.
///
/// The string for the address `addrs[i]` is
/// `std::str::from_utf8(buffer[index[i] as usize .. index[i + 1] as usize])`
pub struct CompactSymbolTable {
    /// A sorted array of symbol addresses, as library-relative offsets in
    /// bytes, in ascending order.
    pub addr: Vec<u32>,
    /// Contains positions into `buffer`. For every address `addr[i]`,
    /// `index[i]` is the position where the string for that address starts in
    /// the buffer. Also contains one extra index at the end which is `buffer.len()`.
    /// `index.len() == addr.len() + 1`
    pub index: Vec<u32>,
    /// A buffer of bytes that contains all strings from this symbol table,
    /// in the order of the addresses they correspond to, in utf-8 encoded
    /// form, all concatenated together.
    pub buffer: Vec<u8>,
}

impl SymbolicationResult for CompactSymbolTable {
    fn from_full_map<T: Deref<Target = str>>(mut entries: Vec<(u32, T)>) -> Self {
        entries.reverse();
        entries.sort_by_key(|(address, _)| *address);
        entries.dedup_by_key(|(address, _)| *address);
        let total_str_len = entries.iter().map(|(_, s)| s.len()).sum();
        let mut addr = Vec::with_capacity(entries.len());
        let mut index = Vec::with_capacity(entries.len() + 1);
        let mut buffer = Vec::with_capacity(total_str_len);
        for (address, name) in entries {
            addr.push(address);
            index.push(buffer.len() as u32);
            buffer.extend_from_slice(name.as_bytes());
        }
        index.push(buffer.len() as u32);
        Self {
            addr,
            index,
            buffer,
        }
    }

    fn for_addresses(_addresses: &[u32]) -> Self {
        panic!("Should not be called")
    }

    fn add_address_symbol(&mut self, _address: u32, _symbol_address: u32, _symbol_name: &str) {
        panic!("Should not be called")
    }

    fn add_address_debug_info(&mut self, _address: u32, _info: AddressDebugInfo) {
        panic!("Should not be called")
    }

    fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {}
}
