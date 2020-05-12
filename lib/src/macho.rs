use crate::compact_symbol_table::CompactSymbolTable;
use crate::error::{GetSymbolsError, Result};
use object::read::{File, Object};
use uuid::Uuid;

pub fn get_compact_symbol_table(buffer: &[u8], breakpad_id: &str) -> Result<CompactSymbolTable> {
    let macho_file =
        File::parse(buffer).or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;

    let macho_id = match macho_file.mach_uuid() {
        Ok(Some(uuid)) => format!("{:X}0", Uuid::from_bytes(uuid).to_simple()),
        _ => {
            return Err(GetSymbolsError::InvalidInputError(
                "Could not get mach uuid",
            ))
        }
    };
    if macho_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            macho_id,
            breakpad_id.to_string(),
        ));
    }
    Ok(CompactSymbolTable::from_object(&macho_file))
}
