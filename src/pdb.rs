use compact_symbol_table::CompactSymbolTable;
use pdb_crate::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

pub fn get_compact_symbol_table(pdb_data: &[u8], breakpad_id: &str) -> Option<CompactSymbolTable> {
    // Now, parse the PDB and check it against the expected breakpad_id.
    let pdb_reader = Cursor::new(pdb_data);
    let mut pdb = PDB::open(pdb_reader).ok()?;
    let info = pdb.pdb_information().ok()?;
    let pdb_id = format!("{}{:x}", format!("{:X}", info.guid.to_simple()), info.age);

    if pdb_id != breakpad_id {
        return None;
    }

    // Now, gather the symbols into a hashmap.
    let addr_map = pdb.address_map().ok()?;

    // Start with the public function symbols.
    let global_symbols = pdb.global_symbols().ok()?;
    let mut hashmap: HashMap<_, _> = global_symbols
        .iter()
        .filter_map(|symbol| match symbol.parse() {
            Ok(SymbolData::PublicSymbol(PublicSymbol {
                function: true,
                offset,
                ..
            })) => Some((offset.to_rva(&addr_map)?.0, symbol.name().ok()?.to_string())),
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
                if let Ok(SymbolData::Procedure(ProcedureSymbol { offset, .. })) = symbol.parse() {
                    let name = symbol.name().ok()?;
                    hashmap
                        .entry(offset.to_rva(&addr_map)?.0)
                        .or_insert_with(|| Cow::from(name.to_string().into_owned()));
                }
            }
        }
    }

    return Some(CompactSymbolTable::from_map(hashmap));
}
