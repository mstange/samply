use crate::error::{GetSymbolsError, Result};
use crate::shared::FileAndPathHelper;
use std::collections::HashMap;

pub mod looked_up_addresses;
pub mod response_json;

use super::request_json::{self, Lib};
use looked_up_addresses::{AddressResult, LookedUpAddresses};
use serde_json::json;

pub async fn query_api_json(request_json: &str, helper: &impl FileAndPathHelper) -> String {
    match query_api_fallible_json(request_json, helper).await {
        Ok(response_json) => response_json,
        Err(err) => json!({ "error": err.to_string() }).to_string(),
    }
}

pub async fn query_api_fallible_json(
    request_json: &str,
    helper: &impl FileAndPathHelper,
) -> Result<String> {
    let request: request_json::Request = serde_json::from_str(request_json)?;
    let response = query_api(&request, helper).await?;
    Ok(serde_json::to_string(&response)?)
}

pub async fn query_api(
    request: &request_json::Request,
    helper: &impl FileAndPathHelper,
) -> Result<response_json::Response> {
    let requested_addresses = gather_requested_addresses(request)?;
    let symbolicated_addresses = symbolicate_requested_addresses(requested_addresses, helper).await;
    Ok(create_response(request, symbolicated_addresses))
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
) -> HashMap<Lib, Result<LookedUpAddresses>> {
    let mut symbolicated_addresses = HashMap::new();
    for (lib, addresses) in requested_addresses.into_iter() {
        let address_results = get_address_results(&lib, addresses, helper).await;
        symbolicated_addresses.insert(lib, address_results);
    }
    symbolicated_addresses
}

async fn get_address_results(
    lib: &Lib,
    mut addresses: Vec<u32>,
    helper: &impl FileAndPathHelper,
) -> Result<LookedUpAddresses> {
    addresses.sort();
    addresses.dedup();
    Ok(
        crate::get_symbolication_result(&lib.debug_name, &lib.breakpad_id, &addresses, helper)
            .await?,
    )
}

fn create_response(
    request: &request_json::Request,
    symbolicated_addresses: HashMap<Lib, Result<LookedUpAddresses>>,
) -> response_json::Response {
    use response_json::{
        DebugInfo, InlineStackFrame, ModuleStatus, Response, Result, Stack, StackFrame, Symbol,
    };

    fn result_for_job(
        job: &request_json::Job,
        symbolicated_addresses: &HashMap<Lib, crate::Result<LookedUpAddresses>>,
    ) -> Result {
        let mut symbols_by_module_index = HashMap::new();
        let module_status: Vec<Option<_>> = job
            .memory_map
            .iter()
            .enumerate()
            .map(|(module_index, lib)| {
                if let Some(symbol_result) = symbolicated_addresses.get(lib) {
                    if let Ok(symbols) = symbol_result {
                        symbols_by_module_index
                            .insert(module_index as u32, &symbols.address_results);
                    }
                    let (found, symbol_count, errors) = match symbol_result {
                        Ok(symbols) => (true, symbols.symbol_count, vec![]),
                        Err(err) => (false, 0, vec![err.into()]),
                    };
                    Some(ModuleStatus {
                        found,
                        symbol_count,
                        errors,
                    })
                } else {
                    None
                }
            })
            .collect();

        let stacks = job.stacks.iter().map(|stack| {
            response_stack_for_request_stack(stack, &job.memory_map, &symbols_by_module_index)
        });

        Result {
            stacks: stacks.collect(),
            module_status,
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
                    debug_info: address_result
                        .inline_frames
                        .as_ref()
                        .map(|frames| DebugInfo {
                            inline_stack: frames
                                .iter()
                                .map(|inline_frame| InlineStackFrame {
                                    function_name: inline_frame.function.clone(),
                                    file_path: inline_frame.file_path.clone(),
                                    line_number: inline_frame.line_number,
                                })
                                .collect(),
                        }),
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
