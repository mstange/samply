use crate::{FileAndPathHelper, GetSymbolsError, Result};
use std::collections::HashMap;

pub mod request_json;
pub mod response_json;
pub mod symbol_table;

use request_json::Lib;
use symbol_table::{AddressResult, SymbolTable};

pub async fn get_api_response(
    request_json_data: &str,
    helper: &impl FileAndPathHelper,
) -> Result<String> {
    let request: request_json::Request = serde_json::from_str(request_json_data)?;
    let requested_addresses = gather_requested_addresses(&request)?;
    let symbolicated_addresses = symbolicate_requested_addresses(requested_addresses, helper).await;
    let response = create_response(&request, symbolicated_addresses);
    Ok(serde_json::to_string_pretty(&response)?)
}

fn gather_requested_addresses(request: &request_json::Request) -> Result<HashMap<Lib, Vec<u32>>> {
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
                GetSymbolsError::ParseRequestErrorContents(
                    "Stack frame module index beyond the memoryMap",
                ),
            )?;
            requested_addresses
                .entry((*lib).clone())
                .or_insert_with(Vec::new)
                .extend(addresses);
        }
    }
    Ok(requested_addresses)
}

async fn symbolicate_requested_addresses(
    requested_addresses: HashMap<Lib, Vec<u32>>,
    helper: &impl FileAndPathHelper,
) -> HashMap<Lib, Option<HashMap<u32, AddressResult>>> {
    let mut symbolicated_addresses: HashMap<Lib, Option<HashMap<u32, AddressResult>>> =
        HashMap::new();
    for (lib, addresses) in requested_addresses.into_iter() {
        let address_results = get_address_results(&lib, addresses, helper).await.ok();
        symbolicated_addresses.insert(lib, address_results);
    }
    symbolicated_addresses
}

async fn get_address_results(
    lib: &Lib,
    addresses: Vec<u32>,
    helper: &impl FileAndPathHelper,
) -> Result<HashMap<u32, AddressResult>> {
    let symbol_table: SymbolTable =
        crate::get_symbol_table_result(&lib.debug_name, &lib.breakpad_id, helper).await?;
    Ok(symbol_table.look_up_addresses(addresses))
}

fn create_response(
    request: &request_json::Request,
    symbolicated_addresses: HashMap<Lib, Option<HashMap<u32, AddressResult>>>,
) -> response_json::Response {
    use response_json::{Response, Result, Stack, StackFrame, Symbol};

    fn result_for_job(
        job: &request_json::Job,
        symbolicated_addresses: &HashMap<Lib, Option<HashMap<u32, AddressResult>>>,
    ) -> Result {
        let mut found_modules = HashMap::new();
        let mut symbols_by_module_index = HashMap::new();
        for (module_index, lib) in job.memory_map.iter().enumerate() {
            if let Some(symbols) = symbolicated_addresses.get(lib) {
                found_modules.insert(
                    format!("{}/{}", lib.debug_name, lib.breakpad_id),
                    symbols.is_some(),
                );
                if let Some(symbols) = symbols {
                    symbols_by_module_index.insert(module_index as u32, symbols);
                }
            }
        }

        let stacks = job.stacks.iter().map(|stack| {
            response_stack_for_request_stack(stack, &job.memory_map, &symbols_by_module_index)
        });

        Result {
            stacks: stacks.collect(),
            found_modules,
        }
    }

    fn response_stack_for_request_stack(
        stack: &request_json::Stack,
        memory_map: &Vec<Lib>,
        symbols_by_module_index: &HashMap<u32, &HashMap<u32, AddressResult>>,
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
        memory_map: &Vec<Lib>,
        symbols_by_module_index: &HashMap<u32, &HashMap<u32, AddressResult>>,
    ) -> StackFrame {
        let symbol = symbols_by_module_index
            .get(&frame.module_index)
            .map(|symbol_map| {
                // If we have a symbol table for this library, then we know that
                // this address is present in it.
                let address_result = symbol_map.get(&frame.address).unwrap();
                Symbol {
                    function: address_result.symbol_name.clone(),
                    function_offset: frame.address - address_result.symbol_address,
                }
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
