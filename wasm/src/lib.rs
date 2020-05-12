use profiler_get_symbols;
use wasm_bindgen;

mod compact_symbol_table;
mod error;
mod wasm_mem_buffer;

use wasm_bindgen::prelude::*;

pub use compact_symbol_table::CompactSymbolTable;
pub use error::GetSymbolsError;
pub use wasm_mem_buffer::WasmMemBuffer;

#[wasm_bindgen]
pub fn get_compact_symbol_table(
    binary_data: &WasmMemBuffer,
    debug_data: &WasmMemBuffer,
    breakpad_id: &str,
) -> std::result::Result<CompactSymbolTable, JsValue> {
    match profiler_get_symbols::get_compact_symbol_table(
        binary_data.get(),
        debug_data.get(),
        breakpad_id,
    ) {
        Result::Ok(table) => Ok(table.into()),
        Result::Err(err) => Err(GetSymbolsError::from(err).into()),
    }
}
