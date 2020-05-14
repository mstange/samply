use crate::error::{Context, GetSymbolsError, Result};
use crate::pdb_crate::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use crate::SymbolTableResult;
use std::borrow::Cow;
use std::collections::HashMap;

pub fn get_symbol_table_result<'s, S, R>(mut pdb: PDB<'s, S>, breakpad_id: &str) -> Result<R>
where
    R: SymbolTableResult,
    S: pdb_crate::Source<'s> + 's,
{
    // Check against the expected breakpad_id.
    let info = pdb.pdb_information().context("pdb_information")?;
    let pdb_id = format!("{}{:x}", format!("{:X}", info.guid.to_simple()), info.age);

    if pdb_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            pdb_id,
            breakpad_id.to_string(),
        ));
    }

    // Now, gather the symbols into a hashmap.
    let addr_map = pdb.address_map().context("address_map")?;

    // Start with the public function symbols.
    let global_symbols = pdb.global_symbols().context("global_symbols")?;
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
        let mut modules = dbi.modules().context("dbi.modules()")?;
        while let Some(module) = modules.next().context("modules.next()")? {
            let info = match pdb.module_info(&module) {
                Ok(info) => info,
                _ => continue,
            };
            let mut symbols = info.symbols().context("info.symbols()")?;
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

    Ok(R::from_map(hashmap))
}
