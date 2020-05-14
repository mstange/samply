use crate::SymbolTableResult;
use std::collections::HashMap;
use std::ops::Deref;

pub struct AddressResult {
    pub symbol_name: String,
    pub symbol_address: u32,
}

pub struct SymbolTable {
    symbols: Vec<(u32, String)>,
}

impl SymbolTableResult for SymbolTable {
    fn from_map<T: Deref<Target = str>>(map: HashMap<u32, T>) -> Self {
        let mut symbols: Vec<_> = map
            .into_iter()
            .map(|(address, symbol)| (address, String::from(&*symbol)))
            .collect();
        symbols.sort_by_key(|&(addr, _)| addr);
        symbols.dedup_by_key(|&mut (addr, _)| addr);
        SymbolTable { symbols }
    }
}

impl SymbolTable {
    pub fn look_up_addresses(&self, mut addresses: Vec<u32>) -> HashMap<u32, AddressResult> {
        addresses.sort();
        addresses.dedup();
        let mut results = HashMap::new();
        for address in addresses.into_iter() {
            let index = match self
                .symbols
                .binary_search_by_key(&address, |&(addr, _)| addr)
            {
                Ok(i) => i as i32,
                Err(i) => i as i32 - 1,
            };
            let (symbol_address, symbol_name) = if index < 0 {
                (address, String::from("<before first symbol>"))
            } else {
                self.symbols[index as usize].clone()
            };
            results.insert(
                address,
                AddressResult {
                    symbol_address,
                    symbol_name,
                },
            );
        }
        results
    }
}
