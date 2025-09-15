use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use samply_symbols::{FileAndPathHelper, FrameDebugInfo, SymbolMap};
use serde::ser::{SerializeMap, SerializeSeq};

use super::request_json::{Job, Lib, RequestFrame, RequestStack};
use crate::api_file_path::to_api_file_path;
use crate::symbolicate::looked_up_addresses::{AddressResults, PathResolver};
use crate::symbolicate::request_json::Request;

pub struct PerLibResult<H: FileAndPathHelper> {
    pub address_results: AddressResults,
    pub symbol_map: Arc<SymbolMap<H>>,
}

pub struct Response<H: FileAndPathHelper> {
    pub request: Request,
    pub per_lib_results: HashMap<Lib, std::result::Result<PerLibResult<H>, samply_symbols::Error>>,
}

impl<H: FileAndPathHelper> serde::Serialize for Response<H> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let request = &self.request;
        let per_lib_results: HashMap<_, _> = self
            .per_lib_results
            .iter()
            .map(|(lib, lr)| {
                let lr_ref = match lr {
                    Ok(lr) => PerLibResultRef {
                        address_results: Ok(&lr.address_results),
                        path_resolver: &*lr.symbol_map,
                    },
                    Err(e) => PerLibResultRef {
                        address_results: Err(e),
                        path_resolver: &(),
                    },
                };
                (lib, lr_ref)
            })
            .collect();
        let per_lib_results = &per_lib_results;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(
            "results",
            &ResponseResults {
                request,
                per_lib_results,
            },
        )?;
        map.end()
    }
}

pub struct PerLibResultRef<'a> {
    pub address_results: std::result::Result<&'a AddressResults, &'a samply_symbols::Error>,
    pub path_resolver: &'a dyn PathResolver,
}

pub struct ResponseResults<'a> {
    pub request: &'a Request,
    pub per_lib_results: &'a HashMap<&'a Lib, PerLibResultRef<'a>>,
}

impl<'a> serde::Serialize for ResponseResults<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for job in self.request.jobs() {
            seq.serialize_element(&Result {
                job,
                per_lib_results: self.per_lib_results,
            })?;
        }
        seq.end()
    }
}

pub struct Result<'a> {
    pub job: &'a Job,
    pub per_lib_results: &'a HashMap<&'a Lib, PerLibResultRef<'a>>,
}

struct PerModuleResultsRef<'a> {
    pub address_results: &'a AddressResults,
    pub path_resolver: &'a dyn PathResolver,
}

impl<'a> serde::Serialize for Result<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut found_modules = HashMap::new();
        let mut module_errors = HashMap::new();
        let mut symbols_by_module_index = HashMap::new();
        for (module_index, lib) in self.job.memory_map.iter().enumerate() {
            if let Some(per_lib_result) = self.per_lib_results.get(lib) {
                let module_key = format!("{}/{}", lib.debug_name, lib.breakpad_id);
                match per_lib_result.address_results {
                    Ok(address_results) => {
                        let module_results = PerModuleResultsRef {
                            address_results,
                            path_resolver: per_lib_result.path_resolver,
                        };
                        symbols_by_module_index.insert(module_index as u32, module_results);
                    }
                    Err(err) => {
                        module_errors.insert(module_key.clone(), vec![Error(err)]);
                    }
                }
                found_modules.insert(module_key, per_lib_result.address_results.is_ok());
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
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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

        if let Some(symbol_map) = self.symbols_by_module_index.get(&frame.module_index) {
            // If we have a symbol table for this library, then we know that
            // this address is present in it.
            let address_result = symbol_map.address_results.get(&frame.address).unwrap();
            // But the result might still be None.
            if let Some(address_result) = address_result {
                map.serialize_entry("function", &address_result.symbol.name)?;
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
                    if let Some(file) = &outer.file_path {
                        let file = symbol_map.path_resolver.resolve_source_file_path(*file);
                        map.serialize_entry("file", &to_api_file_path(&file))?;
                    }
                    if let Some(line) = outer.line_number.and_then(NonZeroU32::new) {
                        map.serialize_entry("line", &line)?;
                    }

                    if !inlines.is_empty() {
                        map.serialize_entry(
                            "inlines",
                            &ResponseInlineFrames(inlines, symbol_map.path_resolver),
                        )?;
                    }
                }
            }
        }

        map.end()
    }
}

struct ResponseInlineFrames<'a>(&'a [FrameDebugInfo], &'a dyn PathResolver);

impl<'a> serde::Serialize for ResponseInlineFrames<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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

struct ResponseInlineFrame<'a>(&'a FrameDebugInfo, &'a dyn PathResolver);

impl<'a> serde::Serialize for ResponseInlineFrame<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        if let Some(function) = &self.0.function {
            map.serialize_entry("function", &function)?;
        }
        if let Some(file) = &self.0.file_path {
            let file = self.1.resolve_source_file_path(*file);
            map.serialize_entry("file", &to_api_file_path(&file))?;
        }
        if let Some(line) = self.0.line_number.and_then(NonZeroU32::new) {
            map.serialize_entry("line", &line)?;
        }
        map.end()
    }
}

struct SerializeAsHexStr<T>(T);

impl<T: std::fmt::LowerHex> serde::Serialize for SerializeAsHexStr<T> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&format_args!("{:#x}", self.0))
    }
}

pub struct Error<'a>(&'a samply_symbols::Error);

impl<'a> serde::Serialize for Error<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self.0)
    }
}
