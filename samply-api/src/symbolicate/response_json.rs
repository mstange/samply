use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;

use samply_symbols::FrameDebugInfo;
use serde::ser::{SerializeMap, SerializeSeq};

use super::looked_up_addresses::{AddressResult, LookedUpAddresses};
use super::request_json::{Job, Lib, RequestFrame, RequestStack};
use crate::api_file_path::to_api_file_path;
use crate::symbolicate::request_json::Request;

pub struct Response(pub Results);

impl serde::Serialize for Response {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("results", &self.0)?;
        map.end()
    }
}

pub struct Results {
    pub request: Request,
    pub symbolicated_addresses:
        HashMap<Lib, std::result::Result<LookedUpAddresses, samply_symbols::Error>>,
}

impl serde::Serialize for Results {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for job in self.request.jobs() {
            seq.serialize_element(&Result {
                job,
                symbolicated_addresses: &self.symbolicated_addresses,
            })?;
        }
        seq.end()
    }
}

pub struct Result<'a> {
    pub job: &'a Job,
    pub symbolicated_addresses:
        &'a HashMap<Lib, std::result::Result<LookedUpAddresses, samply_symbols::Error>>,
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
            if let Some(symbol_result) = self.symbolicated_addresses.get(lib) {
                let module_key = format!("{}/{}", lib.debug_name, lib.breakpad_id);
                match symbol_result {
                    Ok(symbols) => {
                        symbols_by_module_index
                            .insert(module_index as u32, &symbols.address_results);
                    }
                    Err(err) => {
                        module_errors.insert(module_key.clone(), vec![Error(err)]);
                    }
                }
                found_modules.insert(module_key, symbol_result.is_ok());
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
    symbols_by_module_index: &'a HashMap<u32, &'a BTreeMap<u32, Option<AddressResult>>>,
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
    symbols_by_module_index: &'a HashMap<u32, &'a BTreeMap<u32, Option<AddressResult>>>,
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
    symbols_by_module_index: &'a HashMap<u32, &'a BTreeMap<u32, Option<AddressResult>>>,
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
            let address_result = symbol_map.get(&frame.address).unwrap();
            // But the result might still be None.
            if let Some(address_result) = address_result {
                map.serialize_entry("function", &address_result.symbol_name)?;
                map.serialize_entry(
                    "function_offset",
                    &SerializeAsHexStr(frame.address - address_result.symbol_address),
                )?;
                if let Some(function_size) = address_result.function_size {
                    map.serialize_entry("function_size", &SerializeAsHexStr(function_size))?;
                }

                if let Some(frames) = &address_result.inline_frames {
                    let (outer, inlines) = frames
                        .split_last()
                        .expect("inline_frames should always have at least one element");
                    if let Some(file) = &outer.file_path {
                        map.serialize_entry("file", &to_api_file_path(file))?;
                    }
                    if let Some(line) = outer.line_number.and_then(NonZeroU32::new) {
                        map.serialize_entry("line", &line)?;
                    }

                    if !inlines.is_empty() {
                        map.serialize_entry("inlines", &ResponseInlineFrames(inlines))?;
                    }
                }
            }
        }

        map.end()
    }
}

struct ResponseInlineFrames<'a>(&'a [FrameDebugInfo]);

impl<'a> serde::Serialize for ResponseInlineFrames<'a> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for inline in self.0 {
            seq.serialize_element(&ResponseInlineFrame(inline))?;
        }
        seq.end()
    }
}

struct ResponseInlineFrame<'a>(&'a FrameDebugInfo);

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
            map.serialize_entry("file", &to_api_file_path(file))?;
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
