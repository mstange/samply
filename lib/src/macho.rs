use crate::compact_symbol_table::object_to_map;
use crate::error::{GetSymbolsError, Result};
use crate::SymbolicationResult;
use object::read::{File, Object};
use uuid::Uuid;

pub fn get_symbolication_result<R>(buffer: &[u8], breakpad_id: &str, addresses: &[u32]) -> Result<R>
where
    R: SymbolicationResult,
{
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
    let map = object_to_map(&macho_file);
    Ok(R::from_map(map, addresses))
}
