use super::super::demangle;
use crate::shared::{AddressDebugInfo, InlineStackFrame, SymbolicationResult};
use std::collections::HashMap;
use std::ops::Deref;

pub struct AddressResult {
    pub symbol_address: u32,
    pub symbol_name: String,
    pub inline_frames: Option<Vec<InlineStackFrame>>,
}

pub struct LookedUpAddresses {
    pub address_results: HashMap<u32, AddressResult>,
    pub symbol_count: u32,
}

impl SymbolicationResult for LookedUpAddresses {
    fn from_full_map<T: Deref<Target = str>>(map: HashMap<u32, T>, addresses: &[u32]) -> Self {
        let mut symbols: Vec<_> = map.into_iter().collect();
        symbols.sort_by_key(|&(addr, _)| addr);
        let symbol_count = symbols.len() as u32;

        let address_results = addresses
            .iter()
            .map(|&address| {
                let index = match symbols.binary_search_by_key(&address, |&(addr, _)| addr) {
                    Ok(i) => i as i32,
                    Err(i) => i as i32 - 1,
                };
                let (symbol_address, symbol_name) = if index < 0 {
                    (address, String::from("<before first symbol>"))
                } else {
                    let (addr, name) = &symbols[index as usize];
                    (*addr, demangle::demangle_any(&*name))
                };
                (
                    address,
                    AddressResult {
                        symbol_address,
                        symbol_name,
                        inline_frames: None,
                    },
                )
            })
            .collect();
        LookedUpAddresses {
            address_results,
            symbol_count,
        }
    }

    fn from_map_with_addresses<S>(
        map: HashMap<u32, S>,
        addresses: &[u32],
        total_symbol_count: u32,
    ) -> Self
    where
        S: Deref<Target = str>,
    {
        let mut s = Self::from_full_map(map, addresses);
        s.symbol_count = total_symbol_count;
        s
    }

    fn wants_address_debug_info() -> bool {
        true
    }
    fn wants_full_map() -> bool {
        false
    }

    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo) {
        self.address_results
            .get_mut(&address)
            .unwrap()
            .inline_frames = Some(info.frames);
    }
}
