use crate::error::{GetSymbolsError, Result};
use compact_symbol_table::CompactSymbolTable;
use object::{MachOFile, Object};
pub fn get_compact_symbol_table(buffer: &[u8], breakpad_id: &str) -> Result<CompactSymbolTable> {
    let macho_file =
        MachOFile::parse(buffer).or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;
    let macho_id = format!(
        "{:X}0",
        macho_file
            .mach_uuid()
            .ok_or_else(|| GetSymbolsError::InvalidInputError("Could not get mach uuid"))?
            .simple()
    );
    if macho_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            macho_id,
            breakpad_id.to_string(),
        ));
    }
    Ok(CompactSymbolTable::from_object(&macho_file))
}
