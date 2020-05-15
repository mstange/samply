use crate::compact_symbol_table::object_to_map;
use crate::error::{GetSymbolsError, Result};
use crate::{SymbolicationQuery, SymbolicationResult};
use goblin::mach;
use object::read::{File, Object};
use uuid::Uuid;

pub fn get_symbolication_result_multiarch<R>(buffer: &[u8], query: SymbolicationQuery) -> Result<R>
where
    R: SymbolicationResult,
{
    let mut errors = vec![];
    let multi_arch = mach::MultiArch::new(buffer)?;
    for fat_arch in multi_arch.iter_arches().filter_map(std::result::Result::ok) {
        let arch_slice = fat_arch.slice(buffer);
        match get_symbolication_result(arch_slice, query.clone()) {
            Ok(table) => return Ok(table),
            Err(err) => errors.push(err),
        }
    }
    Err(GetSymbolsError::NoMatchMultiArch(errors))
}

pub fn get_symbolication_result<R>(buffer: &[u8], query: SymbolicationQuery) -> Result<R>
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
    let SymbolicationQuery {
        breakpad_id,
        addresses,
        ..
    } = query;
    if macho_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            macho_id,
            breakpad_id.to_string(),
        ));
    }
    let map = object_to_map(&macho_file);
    Ok(R::from_map(map, addresses))
}
