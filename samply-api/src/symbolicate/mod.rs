use std::collections::HashMap;

use samply_symbols::{
    FileAndPathHelper, FramesLookupResult, LibraryInfo, LookupAddress, SymbolManager,
};

use crate::error::Error;
use crate::to_debug_id;

pub mod looked_up_addresses;
pub mod request_json;
pub mod response_json;

use looked_up_addresses::LookedUpAddresses;
use request_json::Lib;
use serde_json::json;

pub struct SymbolicateApi<'a, H: FileAndPathHelper> {
    symbol_manager: &'a SymbolManager<H>,
}

impl<'a, H: FileAndPathHelper> SymbolicateApi<'a, H> {
    /// Create a [`SymbolicateApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<H>) -> Self {
        Self { symbol_manager }
    }

    pub async fn query_api_json(&self, request_json: &str) -> String {
        match self.query_api_fallible_json(request_json).await {
            Ok(response_json) => response_json,
            Err(err) => json!({ "error": err.to_string() }).to_string(),
        }
    }

    pub async fn query_api_fallible_json(&self, request_json: &str) -> Result<String, Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        let response = self.query_api(request).await?;
        Ok(serde_json::to_string(&response)?)
    }

    pub async fn query_api(
        &self,
        request: request_json::Request,
    ) -> Result<response_json::Response, Error> {
        let requested_addresses = gather_requested_addresses(&request)?;
        let symbolicated_addresses = self
            .symbolicate_requested_addresses(requested_addresses)
            .await;
        Ok(create_response(request, symbolicated_addresses))
    }

    async fn symbolicate_requested_addresses(
        &self,
        requested_addresses: HashMap<Lib, Vec<u32>>,
    ) -> HashMap<Lib, Result<LookedUpAddresses, samply_symbols::Error>> {
        let mut symbolicated_addresses = HashMap::new();
        for (lib, addresses) in requested_addresses.into_iter() {
            let address_results = self
                .symbolicate_requested_addresses_for_lib(&lib, addresses)
                .await;
            symbolicated_addresses.insert(lib, address_results);
        }
        symbolicated_addresses
    }

    async fn symbolicate_requested_addresses_for_lib(
        &self,
        lib: &Lib,
        mut addresses: Vec<u32>,
    ) -> Result<LookedUpAddresses, samply_symbols::Error> {
        // Sort the addresses before the lookup, to have a higher chance of hitting
        // the same external file for subsequent addresses.
        addresses.sort_unstable();
        addresses.dedup();

        let debug_id = to_debug_id(&lib.breakpad_id)?;

        let mut symbolication_result = LookedUpAddresses::for_addresses(&addresses);
        let mut external_addresses = Vec::new();

        // Do the synchronous work first, and accumulate external_addresses which need
        // to be handled asynchronously. This allows us to group async file loads by
        // the external file.

        let info = LibraryInfo {
            debug_name: Some(lib.debug_name.to_string()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        let symbol_map = self.symbol_manager.load_symbol_map(&info).await?;

        symbolication_result.set_total_symbol_count(symbol_map.symbol_count() as u32);

        for &address in &addresses {
            if let Some(address_info) = symbol_map.lookup_sync(LookupAddress::Relative(address)) {
                symbolication_result.add_address_symbol(
                    address,
                    address_info.symbol.address,
                    address_info.symbol.name,
                    address_info.symbol.size,
                );
                match address_info.frames {
                    Some(FramesLookupResult::Available(frames)) => {
                        symbolication_result.add_address_debug_info(address, frames)
                    }
                    Some(FramesLookupResult::External(ext_address)) => {
                        external_addresses.push((address, ext_address));
                    }
                    None => {}
                }
            }
        }

        // Look up any addresses whose debug info is in an external file.
        // The symbol_map caches the most recent external file, so we sort our
        // external addresses by ExternalFileAddressRef before we do the lookup,
        // in order to get the best hit rate in lookup_external.
        external_addresses.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));

        for (address, ext_address) in external_addresses {
            if let Some(frames) = symbol_map.lookup_external(&ext_address).await {
                symbolication_result.add_address_debug_info(address, frames);
            }
        }

        Ok(symbolication_result)
    }
}

fn gather_requested_addresses(
    request: &request_json::Request,
) -> Result<HashMap<Lib, Vec<u32>>, Error> {
    let mut requested_addresses: HashMap<Lib, Vec<u32>> = HashMap::new();
    for job in request.jobs() {
        let mut requested_addresses_by_module_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for stack in &job.stacks {
            for frame in &stack.0 {
                requested_addresses_by_module_index
                    .entry(frame.module_index)
                    .or_default()
                    .push(frame.address);
            }
        }
        for (module_index, addresses) in requested_addresses_by_module_index {
            let lib = job.memory_map.get(module_index as usize).ok_or(
                Error::ParseRequestErrorContents("Stack frame module index beyond the memoryMap"),
            )?;
            requested_addresses
                .entry((*lib).clone())
                .or_default()
                .extend(addresses);
        }
    }
    Ok(requested_addresses)
}

fn create_response(
    request: request_json::Request,
    symbolicated_addresses: HashMap<Lib, Result<LookedUpAddresses, samply_symbols::Error>>,
) -> response_json::Response {
    use response_json::{Response, Results};

    Response(Results {
        request,
        symbolicated_addresses,
    })
}
