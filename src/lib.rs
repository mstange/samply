extern crate goblin;
extern crate object;
extern crate pdb as pdb_crate;
extern crate scroll;
extern crate uuid;
extern crate wasm_bindgen;

mod compact_symbol_table;
mod elf;
mod macho;
mod pdb;

use wasm_bindgen::prelude::*;

use std::io::Cursor;
use std::mem;

use goblin::{mach, Hint};

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

fn get_compact_symbol_table_impl(
    binary_data: &[u8],
    debug_data: &[u8],
    breakpad_id: &str,
) -> Option<compact_symbol_table::CompactSymbolTable> {
    let mut reader = Cursor::new(binary_data);
    if let Ok(hint) = goblin::peek(&mut reader) {
        match hint {
            Hint::Elf(_) => {
                return elf::get_compact_symbol_table(binary_data, breakpad_id);
            }
            Hint::Mach(_) => {
                return macho::get_compact_symbol_table(binary_data, breakpad_id);
            }
            Hint::MachFat(_) => {
                let multi_arch = mach::MultiArch::new(binary_data).ok()?;
                for fat_arch in multi_arch.iter_arches().filter_map(Result::ok) {
                    let arch_slice = fat_arch.slice(binary_data);
                    if let Some(table) = macho::get_compact_symbol_table(arch_slice, breakpad_id) {
                        return Some(table);
                    }
                }
            }
            Hint::PE => {
                return pdb::get_compact_symbol_table(binary_data, debug_data, breakpad_id);
            }
            _ => {}
        }
    }
    None
}

#[wasm_bindgen]
pub fn get_compact_symbol_table(
    binary_data: &[u8],
    debug_data: &[u8],
    breakpad_id: &str,
    dest: &mut CompactSymbolTable,
) -> bool {
    match get_compact_symbol_table_impl(binary_data, debug_data, breakpad_id) {
        Some(table) => {
            dest.addr = table.addr;
            dest.index = table.index;
            dest.buffer = table.buffer;
            true
        }
        None => false,
    }
}
