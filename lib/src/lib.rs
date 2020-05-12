extern crate pdb as pdb_crate;

use goblin;

mod compact_symbol_table;
mod elf;
mod error;
mod macho;
mod pdb;

use goblin::{mach, Hint};
use std::io::Cursor;

pub use crate::error::{GetSymbolsError, Result};
pub use crate::compact_symbol_table::CompactSymbolTable;

pub fn get_compact_symbol_table(
    binary_data: &[u8],
    debug_data: &[u8],
    breakpad_id: &str,
) -> Result<CompactSymbolTable> {
    let mut reader = Cursor::new(binary_data);
    match goblin::peek(&mut reader)? {
        Hint::Elf(_) => elf::get_compact_symbol_table(binary_data, breakpad_id),
        Hint::Mach(_) => macho::get_compact_symbol_table(binary_data, breakpad_id),
        Hint::MachFat(_) => {
            let mut errors = vec![];
            let multi_arch = mach::MultiArch::new(binary_data)?;
            for fat_arch in multi_arch.iter_arches().filter_map(std::result::Result::ok) {
                let arch_slice = fat_arch.slice(binary_data);
                match macho::get_compact_symbol_table(arch_slice, breakpad_id) {
                    Ok(table) => return Ok(table),
                    Err(err) => errors.push(err),
                }
            }
            Err(GetSymbolsError::NoMatchMultiArch(errors))
        }
        Hint::PE => pdb::get_compact_symbol_table(debug_data, breakpad_id),
        _ => Err(GetSymbolsError::InvalidInputError(
            "goblin::peek fails to read",
        )),
    }
}
