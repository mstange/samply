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
    fn from_full_map<T: Deref<Target = str>>(
        mut symbols: Vec<(u32, T)>,
        addresses: &[u32],
    ) -> Self {
        symbols.reverse();
        symbols.sort_by_key(|(address, _)| *address);
        symbols.dedup_by_key(|(address, _)| *address);
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
                    Some(AddressResult {
                        symbol_address,
                        symbol_name,
                        inline_frames: None,
                    }),
                )
            })
            .collect();
        LookedUpAddresses {
            address_results,
            symbol_count,
        }
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
        let address_result = self
            .address_results
            .get_mut(&address)
            .unwrap()
            .as_mut()
            .unwrap();

        if let Some(name) = info.frames.last().and_then(|f| f.function.as_ref()) {
            address_result.symbol_name = name.clone();
        }
        address_result.inline_frames = Some(info.frames);
    }

    fn set_total_symbol_count(&mut self, total_symbol_count: u32) {
        self.symbol_count = total_symbol_count;
    }
}
