use crate::error::{GetSymbolsError, Result};
use compact_symbol_table::CompactSymbolTable;
use pdb_crate::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

fn annotate(invocation_description: &'static str) -> impl Fn(pdb::Error) -> GetSymbolsError {
    move |err| GetSymbolsError::PDBError(invocation_description, err)
}

pub fn get_compact_symbol_table(pdb_data: &[u8], breakpad_id: &str) -> Result<CompactSymbolTable> {
    // Now, parse the PDB and check it against the expected breakpad_id.
    let pdb_reader = Cursor::new(pdb_data);
    let mut pdb = PDB::open(pdb_reader)?;
    let info = pdb.pdb_information().map_err(annotate("pdb_information"))?;
    let pdb_id = format!("{}{:x}", format!("{:X}", info.guid.to_simple()), info.age);

    if pdb_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            pdb_id,
            breakpad_id.to_string(),
        ));
    }

    // Now, gather the symbols into a hashmap.
    let addr_map = pdb.address_map().map_err(annotate("address_map"))?;

    // Start with the public function symbols.
    let global_symbols = pdb.global_symbols().map_err(annotate("global_symbols"))?;
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
        .collect()?;

    // Add Procedure symbols from the modules, if present. Some of these might
    // duplicate public symbols; in that case, don't overwrite the existing
    // symbol name because usually the public symbol version has the full
    // function signature whereas the procedure symbol only has the function
    // name itself.
    if let Ok(dbi) = pdb.debug_information() {
        let mut modules = dbi.modules().map_err(annotate("dbi.modules()"))?;
        while let Some(module) = modules.next().map_err(annotate("modules.next()"))? {
            let info = pdb
                .module_info(&module)
                .map_err(annotate("module_info(&module)"))?;
            let mut symbols = info.symbols().map_err(annotate("info.symbols()"))?;
            while let Ok(Some(symbol)) = symbols.next() {
                let offset = match symbol.parse() {
                    Ok(SymbolData::Procedure(ProcedureSymbol { offset, .. })) => offset,
                    _ => continue,
                };
                if let (Ok(name), Some(query)) = (symbol.name(), offset.to_rva(&addr_map)) {
                    hashmap
                        .entry(query.0)
                        .or_insert_with(|| Cow::from(name.to_string().into_owned()));
                }
            }
        }
    }

    Ok(CompactSymbolTable::from_map(hashmap))
}
