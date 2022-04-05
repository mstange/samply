use super::super::demangle;
use crate::shared::{AddressDebugInfo, InlineStackFrame, SymbolicationResult};
use std::collections::BTreeMap;
use std::ops::Deref;

pub struct AddressResult {
    pub symbol_address: u32,
    pub symbol_name: String,
    pub inline_frames: Option<Vec<InlineStackFrame>>,
}

pub type AddressResults = BTreeMap<u32, Option<AddressResult>>;

pub struct LookedUpAddresses {
    pub address_results: AddressResults,
    pub symbol_count: u32,
}

impl SymbolicationResult for LookedUpAddresses {
    fn from_full_map<T: Deref<Target = str>>(_symbols: Vec<(u32, T)>) -> Self {
        panic!("Should not be called")
    }

    fn for_addresses(addresses: &[u32]) -> Self {
        LookedUpAddresses {
            address_results: addresses.iter().map(|&addr| (addr, None)).collect(),
            symbol_count: 0,
        }
    }

    fn add_address_symbol(&mut self, address: u32, symbol_address: u32, symbol_name: &str) {
        *self.address_results.get_mut(&address).unwrap() = Some(AddressResult {
            symbol_address,
            symbol_name: demangle::demangle_any(symbol_name),
            inline_frames: None,
        });
    }

    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo) {
        let outer_function_name = info.frames.last().and_then(|f| f.function.as_deref());
        let entry = self.address_results.get_mut(&address).unwrap();

        match entry {
            Some(address_result) => {
                // Overwrite the symbol name with the function name from the debug info.
                if let Some(name) = outer_function_name {
                    address_result.symbol_name = name.to_string();
                }
                // Add the inline frame info.``
                address_result.inline_frames = Some(info.frames);
            }
            None => {
                // add_address_symbol has not been called for this address.
                // This happens when we only have debug info but no symbol for this address.
                // This is a rare case.
                *entry = Some(AddressResult {
                    symbol_address: address, // TODO: Would be nice to get the actual function start address from addr2line
                    symbol_name: outer_function_name
                        .map_or_else(|| format!("0x{:x}", address), str::to_string),
                    inline_frames: Some(info.frames),
                });
            }
        }
    }

    fn set_total_symbol_count(&mut self, total_symbol_count: u32) {
        self.symbol_count = total_symbol_count;
    }
}
