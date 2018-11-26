use compact_symbol_table::CompactSymbolTable;
use object::{MachOFile, Object};

pub fn get_compact_symbol_table(buffer: &[u8], breakpad_id: &str) -> Option<CompactSymbolTable> {
    let macho_file = MachOFile::parse(buffer).ok()?;
    if format!("{:X}0", macho_file.mach_uuid()?.simple()) != breakpad_id {
        return None;
    }
    return Some(CompactSymbolTable::from_object(&macho_file));
}
