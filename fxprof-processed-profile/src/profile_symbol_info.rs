use crate::string_table::{StringIndex, StringTable};
use crate::LibraryHandle;

/// The container for all symbol info, passed to [`Profile::make_symbolicated_profile`](crate::Profile::make_symbolicated_profile).
pub struct ProfileSymbolInfo {
    /// A table which deduplicates strings for symbol names, function names, and file paths.
    pub string_table: SymbolStringTable,
    /// The actual symbol information, one [`LibSymbolInfo`] per [`LibraryHandle`].
    pub lib_symbols: Vec<LibSymbolInfo>,
}

/// A handle for an index in the [`SymbolStringTable`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SymbolStringIndex(StringIndex);

/// A table which deduplicates strings for symbol names, function names, and file paths.
#[derive(Debug, Clone, Default)]
pub struct SymbolStringTable(StringTable);

impl SymbolStringTable {
    /// Create a new table.
    pub fn new() -> Self {
        Self::default()
    }
    /// Look up or create a [`SymbolStringIndex`] for the specified string.
    pub fn index_for_string(&mut self, s: &str) -> SymbolStringIndex {
        SymbolStringIndex(self.0.index_for_string(s))
    }

    /// Translate a [`SymbolStringIndex`] back into a string.
    pub fn get_string(&self, index: SymbolStringIndex) -> &str {
        self.0.get_string(index.0)
    }
}

/// The symbol information for a single [`LibraryHandle`], stored in [`ProfileSymbolInfo::lib_symbols`].
///
/// Contains one [`AddressInfo`] for each relative address for which symbols were found.
pub struct LibSymbolInfo {
    /// The [`LibraryHandle`], from the unsymbolicated profile, whose symbols are
    /// contained in this object.
    pub lib_handle: LibraryHandle,
    /// The addresses, in ascending order, for which symbols were found.
    pub sorted_addresses: Vec<u32>,
    /// The corresponding [`AddressInfo`] for each address in `sorted_addresses`.
    /// Must have the same length as `sorted_addresses`.
    pub address_infos: Vec<AddressInfo>,
}

/// The symbol information for one symbolicated address within a library, stored
/// in [`LibSymbolInfo::address_infos`].
pub struct AddressInfo {
    /// The name of the "outer function" that this address is in. Usually found
    /// in the actual symbol table of a binary.
    ///
    /// Stored as an index into the [`ProfileSymbolInfo`]'s [`SymbolStringTable`].
    pub symbol_name: SymbolStringIndex,
    /// The address where the "outer function" starts, usually this is the symbol
    /// address from the symbol table of a binary.
    pub symbol_start_address: u32,
    /// The size of the "outer function" if known, in bytes. This tells the assembly
    /// view in the Firefox Profiler UI how many instructions it should read for
    /// this symbol.
    pub symbol_size: Option<u32>,
    /// Detailed information about this address which is usually found in a binary's
    /// debug info. This can contain file + line information and inline frames.
    ///
    /// If empty, the outer function name is taken from `symbol_name`.
    /// If non-empty, this is ordered from "deepest" to "most shallow" frame; the
    /// last element is the outer function.
    pub frames: Vec<AddressFrame>,
}

/// A single frame from the debug info for an address, stored in [`AddressInfo::frames`].
pub struct AddressFrame {
    /// The function name.
    ///
    /// Stored as an index into the [`ProfileSymbolInfo`]'s [`SymbolStringTable`].
    pub function_name: SymbolStringIndex,
    /// The file path.
    ///
    /// Stored as an index into the [`ProfileSymbolInfo`]'s [`SymbolStringTable`].
    pub file: Option<SymbolStringIndex>,
    /// The line number within this function, for our address, if known.
    pub line: Option<u32>,
}
