use std::collections::HashMap;
use std::sync::Arc;

use samply_symbols::{
    AccessPatternHint, FileAndPathHelper, FramesLookupResult, LibraryInfo, LookupAddress,
    SourceFilePath, SourceFilePathHandle, SymbolManager, SymbolMap,
};

use crate::error::Error;
use crate::symbolicate::looked_up_addresses::{AddressResult, AddressResults, PathResolver};
use crate::symbolicate::response_json::{PerLibResult, Response};
use crate::to_debug_id;

pub mod looked_up_addresses;
pub mod request_json;
pub mod response_json;

use request_json::Lib;

impl<H: FileAndPathHelper> PathResolver for SymbolMap<H> {
    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        self.resolve_source_file_path(handle)
    }
}

pub struct SymbolicateApi<'a, H: FileAndPathHelper> {
    symbol_manager: &'a SymbolManager<H>,
}

impl<'a, H: FileAndPathHelper + 'static> SymbolicateApi<'a, H> {
    /// Create a [`SymbolicateApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<H>) -> Self {
        Self { symbol_manager }
    }

    pub async fn query_api_json(
        &self,
        request_json: &str,
    ) -> Result<response_json::Response<H>, Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        self.query_api(request).await
    }

    pub async fn query_api(
        &self,
        request: request_json::Request,
    ) -> Result<response_json::Response<H>, Error> {
        let requested_addresses = gather_requested_addresses(&request)?;
        let per_lib_results = self
            .symbolicate_requested_addresses(requested_addresses)
            .await;
        Ok(Response {
            request,
            per_lib_results,
        })
    }

    async fn symbolicate_requested_addresses(
        &self,
        requested_addresses: HashMap<Lib, Vec<u32>>,
    ) -> HashMap<Lib, Result<PerLibResult<H>, samply_symbols::Error>> {
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
    ) -> Result<PerLibResult<H>, samply_symbols::Error> {
        // Sort the addresses before the lookup, to have a higher chance of hitting
        // the same external file for subsequent addresses.
        addresses.sort_unstable();
        addresses.dedup();

        let debug_id = to_debug_id(&lib.breakpad_id)?;

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
        symbol_map.set_access_pattern_hint(AccessPatternHint::SequentialLookup);

        let mut address_results: AddressResults =
            addresses.iter().map(|&addr| (addr, None)).collect();

        for &address in &addresses {
            if let Some(address_info) = symbol_map.lookup_sync(LookupAddress::Relative(address)) {
                let address_result = address_results.get_mut(&address).unwrap();
                *address_result = Some(AddressResult::new(
                    address_info.symbol.address,
                    address_info.symbol.name,
                    address_info.symbol.size,
                ));
                match address_info.frames {
                    Some(FramesLookupResult::Available(frames)) => {
                        address_result.as_mut().unwrap().set_debug_info(frames)
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
                let address_result = address_results.get_mut(&address).unwrap();
                address_result.as_mut().unwrap().set_debug_info(frames);
            }
        }

        let outcome = PerLibResult {
            address_results,
            symbol_map: Arc::new(symbol_map),
        };

        Ok(outcome)
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
