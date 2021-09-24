use crate::error::{GetSymbolsError, Result};
use crate::shared::{FileAndPathHelper, SymbolicationQuery, SymbolicationResultKind};
use std::collections::HashMap;

pub mod looked_up_addresses;
pub mod response_json;

use super::request_json::{self, Lib};
use looked_up_addresses::{AddressResults, LookedUpAddresses};
use regex::Regex;
use serde_json::json;

pub async fn query_api_json(
    request_json: &str,
    helper: &impl FileAndPathHelper,
    with_debug_info: bool,
) -> String {
    match query_api_fallible_json(request_json, helper, with_debug_info).await {
        Ok(response_json) => response_json,
        Err(err) => json!({ "error": err.to_string() }).to_string(),
    }
}

pub async fn query_api_fallible_json(
    request_json: &str,
    helper: &impl FileAndPathHelper,
    with_debug_info: bool,
) -> Result<String> {
    let request: request_json::Request = serde_json::from_str(request_json)?;
    let response = query_api(&request, helper, with_debug_info).await?;
    Ok(serde_json::to_string(&response)?)
}

pub async fn query_api(
    request: &request_json::Request,
    helper: &impl FileAndPathHelper,
    with_debug_info: bool,
) -> Result<response_json::Response> {
    let requested_addresses = gather_requested_addresses(request)?;
    let symbolicated_addresses =
        symbolicate_requested_addresses(requested_addresses, helper, with_debug_info).await;
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
    with_debug_info: bool,
) -> HashMap<Lib, Result<LookedUpAddresses>> {
    let mut symbolicated_addresses = HashMap::new();
    for (lib, mut addresses) in requested_addresses.into_iter() {
        addresses.sort_unstable();
        addresses.dedup();
        let address_results = crate::get_symbolication_result(
            SymbolicationQuery {
                debug_name: &lib.debug_name,
                breakpad_id: &lib.breakpad_id,
                result_kind: SymbolicationResultKind::SymbolsForAddresses {
                    addresses: &addresses,
                    with_debug_info,
                },
            },
            helper,
        )
        .await;
        symbolicated_addresses.insert(lib, address_results);
    }
    symbolicated_addresses
}

fn create_response(
    request: &request_json::Request,
    symbolicated_addresses: HashMap<Lib, Result<LookedUpAddresses>>,
) -> response_json::Response {
    use response_json::{DebugInfo, InlineStackFrame, Response, Result, Stack, StackFrame, Symbol};

    fn result_for_job<'a>(
        job: &request_json::Job,
        symbolicated_addresses: &'a HashMap<Lib, crate::Result<LookedUpAddresses>>,
        path_mapper: &mut PathMapper<'a>,
    ) -> Result {
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
            response_stack_for_request_stack(
                stack,
                &job.memory_map,
                &symbols_by_module_index,
                path_mapper,
            )
        });

        Result {
            stacks: stacks.collect(),
            found_modules,
            module_errors,
        }
    }

    fn response_stack_for_request_stack<'a>(
        stack: &request_json::Stack,
        memory_map: &[Lib],
        symbols_by_module_index: &HashMap<u32, &'a AddressResults>,
        path_mapper: &mut PathMapper<'a>,
    ) -> Stack {
        let frames = stack.0.iter().enumerate().map(|(frame_index, frame)| {
            response_frame_for_request_frame(
                frame,
                frame_index as u32,
                memory_map,
                symbols_by_module_index,
                path_mapper,
            )
        });
        Stack(frames.collect())
    }

    fn response_frame_for_request_frame<'a>(
        frame: &request_json::StackFrame,
        frame_index: u32,
        memory_map: &[Lib],
        symbols_by_module_index: &HashMap<u32, &'a AddressResults>,
        path_mapper: &mut PathMapper<'a>,
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
                        debug_info: address_result.inline_frames.as_ref().map(|frames| {
                            let (outer, inlines) = frames
                                .split_last()
                                .expect("inline_frames should always have at least one element");
                            DebugInfo {
                                file: outer.file_path.as_ref().map(|p| path_mapper.map_path(p)),
                                line: outer.line_number,
                                inlines: inlines
                                    .iter()
                                    .map(|inline_frame| InlineStackFrame {
                                        function: inline_frame.function.clone(),
                                        file: inline_frame
                                            .file_path
                                            .as_ref()
                                            .map(|p| path_mapper.map_path(p)),
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

    let mut path_mapper = PathMapper::new();
    Response {
        results: request
            .jobs()
            .map(|job| result_for_job(job, &symbolicated_addresses, &mut path_mapper))
            .collect(),
    }
}

#[derive(Debug, Clone)]
struct PathMapper<'a> {
    cache: HashMap<&'a str, String>,
    rustc_regex: Regex,
    cargo_dep_regex: Regex,
}

impl<'a> PathMapper<'a> {
    fn new() -> Self {
        PathMapper {
            cache: HashMap::new(),
            rustc_regex: Regex::new(r"^/rustc/(?P<rev>[0-9a-f]+)\\?/(?P<path>.*)$").unwrap(),
            cargo_dep_regex: Regex::new(r"[/\\]\.cargo[/\\]registry[/\\]src[/\\](?P<registry>[^/\\]+)[/\\](?P<crate>[^/]+)-(?P<version>[0-9]+\.[0-9]+\.[0-9]+)[/\\](?P<path>.*)$").unwrap(),
        }
    }

    fn map_path(&mut self, path: &'a str) -> String {
        // TODO: 2021 edition
        let rustc_regex = &self.rustc_regex;
        let cargo_dep_regex = &self.cargo_dep_regex;
        self.cache
            .entry(path)
            .or_insert_with(|| {
                if let Some(captures) = rustc_regex.captures(path) {
                    let rev = captures.name("rev").unwrap().as_str();
                    let path = captures.name("path").unwrap().as_str();
                    let path = path.replace("\\", "/");
                    format!("git:github.com/rust-lang/rust:{}:{}", path, rev)
                } else if let Some(captures) = cargo_dep_regex.captures(path) {
                    let registry = captures.name("registry").unwrap().as_str();
                    let crate_ = captures.name("crate").unwrap().as_str();
                    let version = captures.name("version").unwrap().as_str();
                    let path = captures.name("path").unwrap().as_str();
                    let path = path.replace("\\", "/");
                    format!("cargo:{}:{}-{}:{}", registry, crate_, version, path)
                } else {
                    path.into()
                }
            })
            .clone()
    }
}
