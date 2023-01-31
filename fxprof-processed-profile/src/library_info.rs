use debugid::DebugId;
use std::{ops::Range, sync::Arc};

/// A library ("binary" / "module" / "DSO") which is loaded into a process.
/// This can be the main executable file or a dynamic library, or any other
/// mapping of executable memory.
///
/// Library information makes after-the-fact symbolication possible: The
/// profile JSON contains raw code addresses, and then the symbols for these
/// addresses get resolved later.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryInfo {
    /// The "actual virtual memory address", in the address space of the process,
    /// where this library's base address is located. The base address is the
    /// address which "relative addresses" are relative to.
    ///
    /// For ELF binaries, the base address is AVMA of the first segment, i.e. of
    /// the mapping created by the first ELF `LOAD` command.
    ///
    /// For mach-O binaries, the base address is the start of the `__TEXT` segment.
    ///
    /// For Windows binaries, the base address is the image load address.
    pub base_avma: u64,
    /// The address range that this mapping occupies in the virtual memory
    /// address space of the process. AVMA = "actual virtual memory address"
    pub avma_range: Range<u64>,
    /// The name of this library that should be displayed in the profiler.
    /// Usually this is the filename of the binary, but it could also be any other
    /// name, such as "\[kernel.kallsyms\]" or "\[vdso\]".
    pub name: String,
    /// The debug name of this library which should be used when looking up symbols.
    /// On Windows this is the filename of the PDB file, on other platforms it's
    /// usually the same as the filename of the binary.
    pub debug_name: String,
    /// The absolute path to the binary file.
    pub path: String,
    /// The absolute path to the debug file. On Linux and macOS this is the same as
    /// the path to the binary file. On Windows this is the path to the PDB file.
    pub debug_path: String,
    /// The debug ID of the library. This lets symbolication confirm that it's
    /// getting symbols for the right file, and it can sometimes allow obtaining a
    /// symbol file from a symbol server.
    pub debug_id: DebugId,
    /// The code ID of the library. This lets symbolication confirm that it's
    /// getting symbols for the right file, and it can sometimes allow obtaining a
    /// symbol file from a symbol server.
    pub code_id: Option<String>,
    /// An optional string with the CPU arch of this library, for example "x86_64",
    /// "arm64", or "arm64e". This is used for macOS system libraries in the dyld
    /// shared cache, in order to avoid loading the wrong cache files, as a
    /// performance optimization. In the past, this was also used to find the
    /// correct sub-binary in a mach-O fat binary. But we now use the debug_id for that
    /// purpose.
    pub arch: Option<String>,
    /// An optional symbol table, for "pre-symbolicating" stack frames.
    ///
    /// Usually, symbolication is something that should happen asynchronously,
    /// because it can be very slow, so the regular way to use the profiler is to
    /// store only frame addresses and no symbols in the profile JSON, and perform
    /// symbolication only once the profile is loaded in the Firefox Profiler UI.
    ///
    /// However, sometimes symbols are only available during recording and are not
    /// easily accessible afterwards. One such example the symbol table of the
    /// Linux kernel: Users with root privileges can access the symbol table of the
    /// currently-running kernel via `/proc/kallsyms`, but we don't want to have
    /// to run the local symbol server with root privileges. So it's easier to
    /// resolve kernel symbols when generating the profile JSON.
    ///
    /// This way of symbolicating does not support file names, line numbers, or
    /// inline frames. It is intended for relatively "small" symbol tables for which
    /// an address lookup is fast.
    pub symbol_table: Option<Arc<SymbolTable>>,
}

/// A symbol table which contains a list of [`Symbol`]s, used in [`LibraryInfo`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
}

impl SymbolTable {
    /// Create a [`SymbolTable`] from a list of [`Symbol`]s.
    pub fn new(mut symbols: Vec<Symbol>) -> Self {
        symbols.sort();
        symbols.dedup_by_key(|symbol| symbol.address);
        Self { symbols }
    }

    /// Look up the symbol for an address. This address is relative to the library's base address.
    pub fn lookup(&self, address: u32) -> Option<&Symbol> {
        let index = match self
            .symbols
            .binary_search_by_key(&address, |symbol| symbol.address)
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(next_i) => next_i - 1,
        };
        let symbol = &self.symbols[index];
        match symbol.size {
            Some(size) if address < symbol.address.saturating_add(size) => Some(symbol),
            Some(_size) => None,
            None => Some(symbol),
        }
    }
}

/// A single symbol from a [`SymbolTable`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol {
    /// The symbol's address, as a "relative address", i.e. relative to the library's base address.
    pub address: u32,
    /// The symbol's size, if known. This is often just set based on the address of the next symbol.
    pub size: Option<u32>,
    /// The symbol name.
    pub name: String,
}
