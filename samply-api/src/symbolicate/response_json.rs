use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use samply_symbols::{FileAndPathHelper, FrameDebugInfo, SymbolMap, SymbolMapTrait};
use serde::ser::{SerializeMap, SerializeSeq};

use super::request_json::{Job, Lib, RequestFrame, RequestStack};
use crate::api_file_path::to_api_file_path;
use crate::symbolicate::looked_up_addresses::AddressResults;
use crate::symbolicate::request_json::Request;

pub struct LibSymbols<H: FileAndPathHelper> {
    pub address_results: AddressResults,
    pub symbol_map: Arc<SymbolMap<H>>,
}

/// The response for a [`Request`].
///
/// Note: You may be tempted to eliminate the type parameter `H` here. But beware!
/// Response is Send if and only if H is Send. You'd need to pick either Send or
/// non-Send if you wanted to type-erase H. But we need to support both cases:
/// - In samply's server, we call wholesym::SymbolManager::query_api(...).await
///   in an async function whose Future must be Send. Making Response non-Send would
///   turn that entire future non-Send.
/// - In profiler-get-symbols, H is non-Send, so we cannot require H: Send here.
///
/// Actually, that last bit may not be true. Maybe SymbolMap<H> is always Send these
/// days? Maybe this deserves another look.
pub struct Response<H: FileAndPathHelper> {
    pub request: Request,
    pub symbols_per_lib: HashMap<Lib, Result<LibSymbols<H>, samply_symbols::Error>>,
}

impl<H: FileAndPathHelper> serde::Serialize for Response<H> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let request = &self.request;
        let symbols_per_lib: HashMap<_, _> = self
            .symbols_per_lib
            .iter()
            .map(|(lib, lr)| {
                let lr_ref = match lr {
                    Ok(lr) => Ok(LibSymbolsRef {
                        address_results: &lr.address_results,
                        symbol_map: &*lr.symbol_map,
                    }),
                    Err(e) => Err(e),
                };
                (lib, lr_ref)
            })
            .collect();
        let symbols_per_lib = &symbols_per_lib;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(
            "results",
            &ResponseResults {
                request,
                symbols_per_lib,
            },
        )?;
        map.end()
    }
}

pub struct LibSymbolsRef<'a> {
    pub address_results: &'a AddressResults,
    pub symbol_map: &'a dyn SymbolMapTrait,
}

pub struct ResponseResults<'a> {
    pub request: &'a Request,
    pub symbols_per_lib: &'a HashMap<&'a Lib, Result<LibSymbolsRef<'a>, &'a samply_symbols::Error>>,
}

impl<'a> serde::Serialize for ResponseResults<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for job in self.request.jobs() {
            seq.serialize_element(&JobResponse {
                job,
                symbols_per_lib: self.symbols_per_lib,
            })?;
        }
        seq.end()
    }
}

pub struct JobResponse<'a> {
    pub job: &'a Job,
    pub symbols_per_lib: &'a HashMap<&'a Lib, Result<LibSymbolsRef<'a>, &'a samply_symbols::Error>>,
}

struct PerModuleResultsRef<'a> {
    pub address_results: &'a AddressResults,
    pub symbol_map: &'a dyn SymbolMapTrait,
}

impl<'a> serde::Serialize for JobResponse<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut found_modules = HashMap::new();
        let mut module_errors = HashMap::new();
        let mut symbols_by_module_index = HashMap::new();
        for (module_index, lib) in self.job.memory_map.iter().enumerate() {
            if let Some(lib_symbols_result) = self.symbols_per_lib.get(lib) {
                let module_key = format!("{}/{}", lib.debug_name, lib.breakpad_id);
                match lib_symbols_result {
                    Ok(lib_symbols) => {
                        let module_results = PerModuleResultsRef {
                            address_results: lib_symbols.address_results,
                            symbol_map: lib_symbols.symbol_map,
                        };
                        symbols_by_module_index.insert(module_index as u32, module_results);
                    }
                    Err(err) => {
                        module_errors.insert(module_key.clone(), vec![Error(err)]);
                    }
                }
                found_modules.insert(module_key, lib_symbols_result.is_ok());
            }
        }

        let stacks = ResponseStacks {
            request_stacks: &self.job.stacks,
            memory_map: &self.job.memory_map,
            symbols_by_module_index: &symbols_by_module_index,
        };

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("stacks", &stacks)?; // Vec<Stack>
        map.serialize_entry("found_modules", &found_modules)?;
        if !module_errors.is_empty() {
            map.serialize_entry("module_errors", &module_errors)?;
        }
        map.end()
    }
}

struct ResponseStacks<'a> {
    request_stacks: &'a [RequestStack],
    memory_map: &'a [Lib],
    symbols_by_module_index: &'a HashMap<u32, PerModuleResultsRef<'a>>,
}

impl<'a> serde::Serialize for ResponseStacks<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.request_stacks.len()))?;
        for request_stack in self.request_stacks {
            seq.serialize_element(&ResponseStack {
                request_stack: &request_stack.0,
                memory_map: self.memory_map,
                symbols_by_module_index: self.symbols_by_module_index,
            })?;
        }
        seq.end()
    }
}

pub struct ResponseStack<'a> {
    request_stack: &'a [RequestFrame],
    memory_map: &'a [Lib],
    symbols_by_module_index: &'a HashMap<u32, PerModuleResultsRef<'a>>,
}

impl<'a> serde::Serialize for ResponseStack<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.request_stack.len()))?;
        for (i, frame) in self.request_stack.iter().enumerate() {
            seq.serialize_element(&ResponseFrame {
                index: i,
                request_frame: frame,
                memory_map: self.memory_map,
                symbols_by_module_index: self.symbols_by_module_index,
            })?;
        }
        seq.end()
    }
}

pub struct ResponseFrame<'a> {
    index: usize,
    request_frame: &'a RequestFrame,
    memory_map: &'a [Lib],
    symbols_by_module_index: &'a HashMap<u32, PerModuleResultsRef<'a>>,
}

impl<'a> serde::Serialize for ResponseFrame<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        let frame = &self.request_frame;

        map.serialize_entry("frame", &self.index)?;
        map.serialize_entry("module_offset", &SerializeAsHexStr(&frame.address))?;
        map.serialize_entry(
            "module",
            &self.memory_map[frame.module_index as usize].debug_name,
        )?;

        if let Some(PerModuleResultsRef {
            symbol_map,
            address_results,
        }) = self.symbols_by_module_index.get(&frame.module_index)
        {
            // If we have a symbol table for this library, then we know that
            // this address is present in it.
            let address_result = address_results.get(&frame.address).unwrap();
            // But the result might still be None.
            if let Some(address_result) = address_result {
                let symbol_name = symbol_map.resolve_symbol_name(address_result.symbol.name);

                // Use the function name from debug info if available, otherwise use the symbol name
                if let Some(f) = address_result.function_name {
                    let function_name = symbol_map.resolve_function_name(f);
                    map.serialize_entry("function", &function_name)?;
                    if symbol_name != function_name {
                        map.serialize_entry("symbol", &symbol_name)?;
                    }
                } else {
                    map.serialize_entry("function", &symbol_name)?;
                };

                map.serialize_entry(
                    "function_offset",
                    &SerializeAsHexStr(frame.address - address_result.symbol.address),
                )?;
                if let Some(function_size) = address_result.symbol.size {
                    map.serialize_entry("function_size", &SerializeAsHexStr(function_size))?;
                }

                if let Some(frames) = &address_result.inline_frames {
                    let (outer, inlines) = frames
                        .split_last()
                        .expect("inline_frames should always have at least one element");
                    ResponseInlineFrame(outer, *symbol_map)
                        .serialize_file_and_line_info::<S>(&mut map)?;

                    if !inlines.is_empty() {
                        map.serialize_entry(
                            "inlines",
                            &ResponseInlineFrames(inlines, *symbol_map),
                        )?;
                    }
                }
            }
        }

        map.end()
    }
}

struct ResponseInlineFrames<'a>(&'a [FrameDebugInfo], &'a dyn SymbolMapTrait);

impl<'a> serde::Serialize for ResponseInlineFrames<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for inline in self.0 {
            seq.serialize_element(&ResponseInlineFrame(inline, self.1))?;
        }
        seq.end()
    }
}

struct ResponseInlineFrame<'a>(&'a FrameDebugInfo, &'a dyn SymbolMapTrait);

impl<'a> ResponseInlineFrame<'a> {
    pub fn serialize_file_and_line_info<S>(&self, map: &mut S::SerializeMap) -> Result<(), S::Error>
    where
        S: serde::Serializer,
    {
        if let Some(file) = &self.0.file_path {
            let file = self.1.resolve_source_file_path(*file);
            map.serialize_entry("file", &to_api_file_path(&file))?;
        }
        if let Some(line) = self.0.line_number.and_then(NonZeroU32::new) {
            map.serialize_entry("line", &line)?;
        }
        if let Some(col) = self.0.column_number.and_then(NonZeroU32::new) {
            map.serialize_entry("col", &col)?;
        }
        if let Some(function_start_line) = self.0.function_start_line.and_then(NonZeroU32::new) {
            map.serialize_entry("function_start_line", &function_start_line)?;
        }
        if let Some(function_start_col) = self.0.function_start_column.and_then(NonZeroU32::new) {
            map.serialize_entry("function_start_col", &function_start_col)?;
        }
        Ok(())
    }
}
impl<'a> serde::Serialize for ResponseInlineFrame<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        if let Some(function) = self.0.function {
            let function_name = self.1.resolve_function_name(function);
            map.serialize_entry("function", &function_name)?;
        }
        self.serialize_file_and_line_info::<S>(&mut map)?;
        map.end()
    }
}

struct SerializeAsHexStr<T>(T);

impl<T: std::fmt::LowerHex> serde::Serialize for SerializeAsHexStr<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&format_args!("{:#x}", self.0))
    }
}

pub struct Error<'a>(&'a samply_symbols::Error);

impl<'a> serde::Serialize for Error<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", self.0.enum_as_string())?;
        map.serialize_entry("message", &DisplayWrapper(&self.0))?;
        map.end()
    }
}

struct DisplayWrapper<'a, T>(&'a T);

impl<T: std::fmt::Display> serde::Serialize for DisplayWrapper<'_, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self.0)
    }
}
