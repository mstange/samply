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
    fn from_full_map<T: Deref<Target = str>>(_symbols: Vec<(u32, T)>) -> Self {
        panic!("Should not be called")
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
