use std::collections::BTreeMap;

use samply_symbols::FrameDebugInfo;

pub struct AddressResult {
    pub symbol_address: u32,
    pub symbol_name: String,
    pub function_size: Option<u32>,
    pub inline_frames: Option<Vec<FrameDebugInfo>>,
}

pub type AddressResults = BTreeMap<u32, Option<AddressResult>>;

pub struct LookedUpAddresses {
    pub address_results: AddressResults,
    pub symbol_count: u32,
}

impl LookedUpAddresses {
    pub fn for_addresses(addresses: &[u32]) -> Self {
        LookedUpAddresses {
            address_results: addresses.iter().map(|&addr| (addr, None)).collect(),
            symbol_count: 0,
        }
    }

    pub fn add_address_symbol(
        &mut self,
        address: u32,
        symbol_address: u32,
        symbol_name: String,
        function_size: Option<u32>,
    ) {
        *self.address_results.get_mut(&address).unwrap() = Some(AddressResult {
            symbol_address,
            symbol_name,
            function_size,
            inline_frames: None,
        });
    }

    pub fn add_address_debug_info(&mut self, address: u32, frames: Vec<FrameDebugInfo>) {
        let outer_function_name = frames.last().and_then(|f| f.function.as_deref());
        let entry = self.address_results.get_mut(&address).unwrap();

        match entry {
            Some(address_result) => {
                // Overwrite the symbol name with the function name from the debug info.
                if let Some(name) = outer_function_name {
                    address_result.symbol_name = name.to_string();
                }
                // Add the inline frame info.``
                address_result.inline_frames = Some(frames);
            }
            None => {
                // add_address_symbol has not been called for this address.
                // This happens when we only have debug info but no symbol for this address.
                // This is a rare case.
                *entry = Some(AddressResult {
                    symbol_address: address, // TODO: Would be nice to get the actual function start address from addr2line
                    symbol_name: outer_function_name
                        .map_or_else(|| format!("0x{address:x}"), str::to_string),
                    function_size: None,
                    inline_frames: Some(frames),
                });
            }
        }
    }

    pub fn set_total_symbol_count(&mut self, total_symbol_count: u32) {
        self.symbol_count = total_symbol_count;
    }
}
