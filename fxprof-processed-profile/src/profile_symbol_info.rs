use crate::{string_table::{StringIndex, StringTable}, LibraryHandle};


pub struct ProfileSymbolInfo {
    pub function_names: FunctionNameStringTable,
    pub files: FileStringTable,
    pub lib_symbols: Vec<ProfileLibSymbolInfo>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FunctionNameStringIndex(pub StringIndex);

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FileStringIndex(pub StringIndex);

pub struct FunctionNameStringTable(pub StringTable);

pub struct FileStringTable(pub StringTable);

pub struct ProfileLibSymbolInfo {
    pub lib_handle: LibraryHandle,
    pub sorted_addresses: Vec<u32>,
    pub address_infos: Vec<AddressInfo>,
}

pub struct AddressInfo {
    pub symbol_name: FunctionNameStringIndex,
    pub symbol_start_address: u32,
    pub symbol_size: Option<u32>,
    /// If empty, the outer function name is taken from `symbol_name`.
    /// If non-empty, the first element is the outer function.
    pub frames: Vec<AddressFrame>,
}

pub struct AddressFrame {
    pub function_name: FunctionNameStringIndex,
    pub file: FileStringIndex,
    pub line: Option<u32>,
}