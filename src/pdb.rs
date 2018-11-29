use compact_symbol_table::CompactSymbolTable;
use pdb_crate::{FallibleIterator, PDB};
use std::collections::HashMap;
use std::io::Cursor;

pub fn get_compact_symbol_table(buffer: &[u8], breakpad_id: &str) -> Option<CompactSymbolTable> {
    let reader = Cursor::new(buffer);
    let mut pdb = PDB::open(reader).ok()?;
    let info = pdb.pdb_information().ok()?;
    let id = format!(
        "{}{:x}",
        format!("{}", info.guid.simple()).to_uppercase(),
        info.age - 1
    );

    if id != breakpad_id {
        return None;
    }

    let symbol_table = pdb.global_symbols().ok()?;
    let mut hashmap = HashMap::new();

    let mut symbols = symbol_table.iter();
    while let Some(symbol) = symbols.next().ok()? {
        if let Ok(pdb::SymbolData::PublicSymbol(data)) = symbol.parse() {
            hashmap.insert(data.offset, symbol.name().ok()?.to_string());
        }
    }
    return Some(CompactSymbolTable::from_map(hashmap));
}
