use super::super::demangle;
use crate::shared::{AddressDebugInfo, SymbolicationResult};
use std::collections::BTreeMap;
use std::ops::Deref;

pub struct AddressResult {
    pub symbol_name: String,
    pub symbol_address: u32,
}

pub type AddressResults = BTreeMap<u32, Option<AddressResult>>;

pub struct LookedUpAddresses {
    pub address_results: AddressResults,
}

impl SymbolicationResult for LookedUpAddresses {
    fn from_full_map<T: Deref<Target = str>>(
        mut symbols: Vec<(u32, T)>,
        addresses: &[u32],
    ) -> Self {
        symbols.reverse();
        symbols.sort_by_key(|(address, _)| *address);
        symbols.dedup_by_key(|(address, _)| *address);

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
                    Some(AddressResult {
                        symbol_address,
                        symbol_name,
                    }),
                )
            })
            .collect();
        LookedUpAddresses { address_results }
    }

    fn for_addresses(addresses: &[u32]) -> Self {
        LookedUpAddresses {
            address_results: addresses.iter().map(|&addr| (addr, None)).collect(),
        }
    }

    fn add_address_symbol(&mut self, address: u32, symbol_address: u32, symbol_name: &str) {
        *self.address_results.get_mut(&address).unwrap() = Some(AddressResult {
            symbol_address,
            symbol_name: demangle::demangle_any(symbol_name),
        });
    }

    fn add_address_debug_info(&mut self, _address: u32, _info: AddressDebugInfo) {
        panic!("Should not be called")
    }
    fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {}
}
