use std::collections::HashMap;
use std::sync::Arc;

use samply_symbols::{
    AccessPatternHint, FileAndPathHelper, FramesLookupResult, LibraryInfo, LookupAddress,
    SymbolManager,
};

use crate::error::Error;
use crate::symbolicate::looked_up_addresses::{AddressResult, AddressResults};
use crate::symbolicate::response_json::{LibSymbols, Response};
use crate::{to_debug_id, ModuleLoadOutcome, ModuleStat, SymbolicateStats};

pub mod looked_up_addresses;
pub mod request_json;
pub mod response_json;

use request_json::Lib;

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

        let jobs_count = request.jobs().count();
        let stacks_count = request.jobs().map(|j| j.stacks.len()).sum();
        let frames_count = request
            .jobs()
            .map(|j| j.stacks.iter().map(|s| s.0.len()).sum::<usize>())
            .sum();

        let (symbols_per_lib, module_stats) = self
            .symbolicate_requested_addresses(requested_addresses)
            .await;
        Ok(Response {
            request,
            symbols_per_lib,
            stats: SymbolicateStats {
                jobs_count,
                stacks_count,
                frames_count,
                module_stats,
            },
        })
    }

    async fn symbolicate_requested_addresses(
        &self,
        requested_addresses: HashMap<Lib, Vec<u32>>,
    ) -> (
        HashMap<Lib, Result<LibSymbols<H>, samply_symbols::Error>>,
        Vec<ModuleStat>,
    ) {
        let mut symbolicated_addresses = HashMap::new();
        let mut module_stats = Vec::with_capacity(requested_addresses.len());
        for (lib, addresses) in requested_addresses.into_iter() {
            let address_results = self
                .symbolicate_requested_addresses_for_lib(&lib, &addresses)
                .await;
            let outcome = match &address_results {
                Ok(_) => ModuleLoadOutcome::Loaded,
                Err(e) => ModuleLoadOutcome::Failed {
                    error_name: e.enum_as_string(),
                },
            };
            module_stats.push(ModuleStat {
                debug_name: lib.debug_name.to_string(),
                breakpad_id: lib.breakpad_id.to_string(),
                outcome,
            });
            symbolicated_addresses.insert(lib, address_results);
        }
        (symbolicated_addresses, module_stats)
    }

    async fn symbolicate_requested_addresses_for_lib(
        &self,
        lib: &Lib,
        addresses: &[u32],
    ) -> Result<LibSymbols<H>, samply_symbols::Error> {
        let debug_id = to_debug_id(lib.breakpad_id.as_str())?;

        let info = LibraryInfo {
            debug_name: Some(lib.debug_name.to_string()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        let symbol_map = self.symbol_manager.load_symbol_map(&info).await?;

        // Create a BTreeMap for the lookup results. This lets us iterate over the
        // addresses in ascending order - the sort happens when the map is created.
        let mut address_results: AddressResults =
            addresses.iter().map(|&addr| (addr, None)).collect();
        symbol_map.set_access_pattern_hint(AccessPatternHint::SequentialLookup);

        // Do the synchronous work first, and accumulate external_addresses which need
        // to be handled asynchronously. This allows us to group async file loads by
        // the external file.
        let mut external_addresses = Vec::new();

        for (&address, address_result) in &mut address_results {
            let Some(address_info) = symbol_map.lookup_sync(LookupAddress::Relative(address))
            else {
                continue;
            };
            *address_result = Some(AddressResult::new(address_info.symbol));
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

        let outcome = LibSymbols {
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
