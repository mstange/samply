extern crate goblin;
extern crate js_sys;
extern crate object;
extern crate pdb as pdb_crate;
extern crate scroll;
extern crate serde;
extern crate serde_derive;
extern crate uuid;
extern crate wasm_bindgen;

mod compact_symbol_table;
mod elf;
mod error;
mod macho;
mod pdb;

use wasm_bindgen::prelude::*;

use crate::error::{GetSymbolsError, GetSymbolsErrorJson, Result};
use goblin::{mach, Hint};
use std::io::Cursor;
use std::mem;

#[wasm_bindgen]
pub struct CompactSymbolTable {
    addr: Vec<u32>,
    index: Vec<u32>,
    buffer: Vec<u8>,
}

#[wasm_bindgen]
impl CompactSymbolTable {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            addr: vec![],
            index: vec![],
            buffer: vec![],
        }
    }

    pub fn take_addr(&mut self) -> Vec<u32> {
        mem::replace(&mut self.addr, vec![])
    }
    pub fn take_index(&mut self) -> Vec<u32> {
        mem::replace(&mut self.index, vec![])
    }
    pub fn take_buffer(&mut self) -> Vec<u8> {
        mem::replace(&mut self.buffer, vec![])
    }
}

/// WasmMemBuffer lets you allocate a chunk of memory on the wasm heap and
/// directly initialize it from JS without a copy. The constructor takes the
/// allocation size and a callback function which does the initialization.
/// This is useful if you need to get very large amounts of data from JS into
/// wasm (for example, the contents of a 1.7GB libxul.so).
#[wasm_bindgen]
pub struct WasmMemBuffer {
    buffer: Vec<u8>,
}

#[wasm_bindgen]
impl WasmMemBuffer {
    /// Create the buffer and initialize it synchronously in the callback function.
    /// f is called with one argument: the Uint8Array that wraps our buffer.
    /// f should not return anything; its return value is ignored.
    /// f must not call any exported wasm functions! Anything that causes the
    /// wasm heap to resize will invalidate the typed array's internal buffer!
    /// Do not hold on to the array that is passed to f after f completes.
    #[wasm_bindgen(constructor)]
    pub fn new(byte_length: u32, f: &js_sys::Function) -> Self {
        // See https://github.com/rustwasm/wasm-bindgen/issues/1643 for how
        // to improve this method.
        let mut buffer = vec![0; byte_length as usize];
        unsafe {
            // Let JavaScript fill the buffer without making a copy.
            // We give the callback function access to the wasm memory via a
            // JS Uint8Array which wraps the underlying wasm memory buffer at
            // the appropriate offset and length.
            // The callback function is supposed to mutate the contents of
            // buffer. However, the "&mut" here is a bit of a lie:
            // Uint8Array::view takes an immutable reference to a slice, not a
            // mutable one. This is rather sketchy but seems to work for now.
            // https://github.com/rustwasm/wasm-bindgen/issues/1079#issuecomment-508577627
            let array = js_sys::Uint8Array::view(&mut buffer);
            f.call1(&JsValue::NULL, &JsValue::from(array))
                .expect("The callback function should not throw");
        }
        Self { buffer }
    }
}

fn get_compact_symbol_table_impl(
    binary_data: &[u8],
    debug_data: &[u8],
    breakpad_id: &str,
) -> Result<compact_symbol_table::CompactSymbolTable> {
    let mut reader = Cursor::new(binary_data);
    match goblin::peek(&mut reader)? {
        Hint::Elf(_) => elf::get_compact_symbol_table(binary_data, breakpad_id),
        Hint::Mach(_) => macho::get_compact_symbol_table(binary_data, breakpad_id),
        Hint::MachFat(_) => {
            let mut first_error = None;
            let multi_arch = mach::MultiArch::new(binary_data)?;
            for fat_arch in multi_arch.iter_arches().filter_map(std::result::Result::ok) {
                let arch_slice = fat_arch.slice(binary_data);
                match macho::get_compact_symbol_table(arch_slice, breakpad_id) {
                    Ok(table) => return Ok(table),
                    Err(err) => first_error = Some(err),
                }
            }
            Err(first_error.unwrap_or_else(|| {
                GetSymbolsError::InvalidInputError("Incompatible system architecture")
            }))
        }
        Hint::PE => pdb::get_compact_symbol_table(debug_data, breakpad_id),
        _ => Err(GetSymbolsError::InvalidInputError(
            "goblin::peek fails to read",
        )),
    }
}

#[wasm_bindgen]
pub fn get_compact_symbol_table(
    binary_data: &WasmMemBuffer,
    debug_data: &WasmMemBuffer,
    breakpad_id: &str,
) -> std::result::Result<CompactSymbolTable, JsValue> {
    match get_compact_symbol_table_impl(&binary_data.buffer, &debug_data.buffer, breakpad_id) {
        Result::Ok(table) => Ok(CompactSymbolTable {
            addr: table.addr,
            index: table.index,
            buffer: table.buffer,
        }),
        Result::Err(err) => {
            let result = GetSymbolsErrorJson::from_error(err);
            Err(JsValue::from_serde(&result).unwrap())
        }
    }
}
