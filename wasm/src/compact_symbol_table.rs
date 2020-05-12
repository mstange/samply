use std::mem;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Default)]
pub struct CompactSymbolTable {
    addr: Vec<u32>,
    index: Vec<u32>,
    buffer: Vec<u8>,
}

#[wasm_bindgen]
impl CompactSymbolTable {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        CompactSymbolTable::default()
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

impl From<profiler_get_symbols::CompactSymbolTable> for CompactSymbolTable {
    fn from(table: profiler_get_symbols::CompactSymbolTable) -> Self {
        CompactSymbolTable {
            addr: table.addr,
            index: table.index,
            buffer: table.buffer,
        }
    }
}