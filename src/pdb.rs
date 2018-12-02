use compact_symbol_table::CompactSymbolTable;
use goblin::pe::PE;
use pdb_crate::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use scroll::Pread;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;
use uuid::Uuid;

// from https://github.com/m4b/goblin/issues/79, with some changes
fn sig_to_uuid(sig: &[u8; 16]) -> Option<Uuid> {
    Uuid::from_fields(
        sig.pread_with::<u32>(0, scroll::LE).ok()?,
        sig.pread_with::<u16>(4, scroll::LE).ok()?,
        sig.pread_with::<u16>(6, scroll::LE).ok()?,
        &sig[8..],
    )
    .ok()
}

fn get_pe_breakpad_id(pe: &PE) -> Option<String> {
    let debug_info = &pe.debug_data?.codeview_pdb70_debug_info?;
    let uuid = sig_to_uuid(&debug_info.signature)?;
    Some(format!(
        "{}{:x}",
        format!("{}", uuid.simple()).to_uppercase(),
        debug_info.age
    ))
}

pub fn get_compact_symbol_table(
    binary_data: &[u8],
    pdb_data: &[u8],
    breakpad_id: &str,
) -> Option<CompactSymbolTable> {
    // First, parse the binary and check it against the expected breakpad_id.
    let pe = PE::parse(&binary_data).ok()?;
    let pe_id = get_pe_breakpad_id(&pe)?;
    if pe_id != breakpad_id {
        return None;
    }

    let sections = pe.sections;

    // Now, parse the PDB and check it against the expected breakpad_id.
    let pdb_reader = Cursor::new(pdb_data);
    let mut pdb = PDB::open(pdb_reader).ok()?;
    let info = pdb.pdb_information().ok()?;
    let pdb_id = format!(
        "{}{:x}",
        format!("{}", info.guid.simple()).to_uppercase(),
        info.age
    );

    if pdb_id != breakpad_id {
        return None;
    }

    // Now, gather the symbols into a hashmap.

    // Start with the public function symbols.
    let global_symbols = pdb.global_symbols().ok()?;
    let mut hashmap: HashMap<_, _> = global_symbols
        .iter()
        .filter_map(|symbol| match symbol.parse() {
            Ok(SymbolData::PublicSymbol(PublicSymbol {
                function: true,
                offset,
                segment,
                ..
            })) => Some((
                sections[segment as usize - 1].virtual_address + offset,
                symbol.name().ok()?.to_string(),
            )),
            _ => None,
        })
        .collect()
        .ok()?;

    // Add Procedure symbols from the modules, if present. Some of these might
    // duplicate public symbols; in that case, don't overwrite the existing
    // symbol name because usually the public symbol version has the full
    // function signature whereas the procedure symbol only has the function
    // name itself.
    if let Ok(dbi) = pdb.debug_information() {
        let mut modules = dbi.modules().ok()?;
        while let Some(module) = modules.next().ok()? {
            let info = pdb.module_info(&module).ok()?;
            let mut symbols = info.symbols().ok()?;
            while let Some(symbol) = symbols.next().ok()? {
                if let Ok(SymbolData::Procedure(ProcedureSymbol {
                    offset, segment, ..
                })) = symbol.parse()
                {
                    let name = symbol.name().ok()?;
                    hashmap
                        .entry(sections[segment as usize - 1].virtual_address + offset)
                        .or_insert_with(|| Cow::from(name.to_string().into_owned()));
                }
            }
        }
    }

    return Some(CompactSymbolTable::from_map(hashmap));
}
