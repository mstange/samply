use crate::to_debug_id;
use crate::{api_file_path::to_api_file_path, error::Error};
use samply_symbols::{FileAndPathHelper, FramesLookupResult, LibraryInfo, SymbolManager};
use std::collections::HashMap;

pub mod looked_up_addresses;
pub mod request_json;
pub mod response_json;

use looked_up_addresses::{AddressResults, LookedUpAddresses};
use request_json::Lib;
use serde_json::json;

pub struct SymbolicateApi<'a, 'h: 'a, H: FileAndPathHelper<'h>> {
    symbol_manager: &'a SymbolManager<'h, H>,
}

impl<'a, 'h: 'a, H: FileAndPathHelper<'h>> SymbolicateApi<'a, 'h, H> {
    /// Create a [`SymbolicateApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<'h, H>) -> Self {
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
        let response = self.query_api(&request).await?;
        Ok(serde_json::to_string(&response)?)
    }

    pub async fn query_api(
        &self,
        request: &request_json::Request,
    ) -> Result<response_json::Response, Error> {
        let requested_addresses = gather_requested_addresses(request)?;
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
        let debug_file_location;

        // Do the synchronous work first, and keep the symbol_map in a scope without
        // any other await calls so that the Rust compiler can see that the symbol
        // map does not exist across any await calls. This makes it so that the
        // future defined by this async function is Send even if the symbol map is
        // not Send.
        {
            let info = LibraryInfo {
                debug_name: Some(lib.debug_name.to_string()),
                debug_id: Some(debug_id),
                ..Default::default()
            };
            let symbol_map = self.symbol_manager.load_symbol_map(&info).await?;
            debug_file_location = symbol_map.debug_file_location().clone();

            symbolication_result.set_total_symbol_count(symbol_map.symbol_count() as u32);

            for &address in &addresses {
                if let Some(address_info) = symbol_map.lookup(address) {
                    symbolication_result.add_address_symbol(
                        address,
                        address_info.symbol.address,
                        address_info.symbol.name,
                        address_info.symbol.size,
                    );
                    match address_info.frames {
                        FramesLookupResult::Available(frames) => {
                            symbolication_result.add_address_debug_info(address, frames)
                        }
                        FramesLookupResult::External(ext_address) => {
                            external_addresses.push((address, ext_address));
                        }
                        FramesLookupResult::Unavailable => {}
                    }
                }
            }
        }

        // Look up any addresses whose debug info is in an external file.
        // The symbol_manager caches the most recent external file.
        // Since our addresses are sorted, they usually happen to be grouped by external
        // file, so in practice we don't do much (if any) repeated reading of the same
        // external file.

        for (address, ext_address) in external_addresses {
            if let Some(frames) = self
                .symbol_manager
                .lookup_external(&debug_file_location, &ext_address)
                .await
            {
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
                    .or_insert_with(Vec::new)
                    .push(frame.address);
            }
        }
        for (module_index, addresses) in requested_addresses_by_module_index {
            let lib = job.memory_map.get(module_index as usize).ok_or(
                Error::ParseRequestErrorContents("Stack frame module index beyond the memoryMap"),
            )?;
            requested_addresses
                .entry((*lib).clone())
                .or_insert_with(Vec::new)
                .extend(addresses);
        }
    }
    Ok(requested_addresses)
}

fn create_response(
    request: &request_json::Request,
    symbolicated_addresses: HashMap<Lib, Result<LookedUpAddresses, samply_symbols::Error>>,
) -> response_json::Response {
    use response_json::{DebugInfo, FrameDebugInfo, Response, Stack, StackFrame, Symbol};

    fn result_for_job(
        job: &request_json::Job,
        symbolicated_addresses: &HashMap<Lib, Result<LookedUpAddresses, samply_symbols::Error>>,
    ) -> response_json::Result {
        let mut found_modules = HashMap::new();
        let mut module_errors = HashMap::new();
        let mut symbols_by_module_index = HashMap::new();
        for (module_index, lib) in job.memory_map.iter().enumerate() {
            if let Some(symbol_result) = symbolicated_addresses.get(lib) {
                let module_key = format!("{}/{}", lib.debug_name, lib.breakpad_id);
                match symbol_result {
                    Ok(symbols) => {
                        symbols_by_module_index
                            .insert(module_index as u32, &symbols.address_results);
                    }
                    Err(err) => {
                        module_errors.insert(module_key.clone(), vec![err.into()]);
                    }
                }
                found_modules.insert(module_key, symbol_result.is_ok());
            }
        }

        let stacks = job.stacks.iter().map(|stack| {
            response_stack_for_request_stack(stack, &job.memory_map, &symbols_by_module_index)
        });

        response_json::Result {
            stacks: stacks.collect(),
            found_modules,
            module_errors,
        }
    }

    fn response_stack_for_request_stack(
        stack: &request_json::Stack,
        memory_map: &[Lib],
        symbols_by_module_index: &HashMap<u32, &AddressResults>,
    ) -> Stack {
        let frames = stack.0.iter().enumerate().map(|(frame_index, frame)| {
            response_frame_for_request_frame(
                frame,
                frame_index as u32,
                memory_map,
                symbols_by_module_index,
            )
        });
        Stack(frames.collect())
    }

    fn response_frame_for_request_frame(
        frame: &request_json::StackFrame,
        frame_index: u32,
        memory_map: &[Lib],
        symbols_by_module_index: &HashMap<u32, &AddressResults>,
    ) -> StackFrame {
        let symbol = symbols_by_module_index
            .get(&frame.module_index)
            .and_then(|symbol_map| {
                // If we have a symbol table for this library, then we know that
                // this address is present in it.
                symbol_map
                    .get(&frame.address)
                    .unwrap()
                    .as_ref()
                    .map(|address_result| Symbol {
                        function: address_result.symbol_name.clone(),
                        function_offset: frame.address - address_result.symbol_address,
                        function_size: address_result.function_size,
                        debug_info: address_result.inline_frames.as_ref().map(|frames| {
                            let (outer, inlines) = frames
                                .split_last()
                                .expect("inline_frames should always have at least one element");
                            DebugInfo {
                                file: outer.file_path.as_ref().map(to_api_file_path),
                                line: outer.line_number,
                                inlines: inlines
                                    .iter()
                                    .map(|inline_frame| FrameDebugInfo {
                                        function: inline_frame.function.clone(),
                                        file: inline_frame.file_path.as_ref().map(to_api_file_path),
                                        line: inline_frame.line_number,
                                    })
                                    .collect(),
                            }
                        }),
                    })
            });
        StackFrame {
            frame: frame_index,
            module_offset: frame.address,
            module: memory_map[frame.module_index as usize].debug_name.clone(),
            symbol,
        }
    }

    Response {
        results: request
            .jobs()
            .map(|job| result_for_job(job, &symbolicated_addresses))
            .collect(),
    }
}
