//! `wholesym` is a fully-featured library for fetching symbol files and for
//! resolving code addresses to symbols and debug info. It supports Windows, macOS
//! and Linux. It is lightning-fast and optimized for minimal time-to-first-symbol.
//!
//! Use it as follows:
//!
//!  1. Create a [`SymbolManager`] using [`SymbolManager::with_config`].
//!  2. Load a [`SymbolMap`] with [`SymbolManager::load_symbol_map_for_binary_at_path`].
//!  3. Look up an address with [`SymbolMap::lookup`].
//!  4. Inspect the returned [`AddressInfo`], which gives you the symbol name, and
//!     potentially file and line information, along with inlined function info.
//!
//! Behind the scenes, `wholesym` loads symbol files much like a debugger would.
//! It supports symbol servers, collecting information from multiple files, and
//! all kinds of different ways to embed symbol information in various file formats.
//!
//! # Example
//!
//! ```
//! use wholesym::{SymbolManager, SymbolManagerConfig, LookupAddress};
//! use std::path::Path;
//!
//! # async fn run() -> Result<(), wholesym::Error> {
//! let symbol_manager = SymbolManager::with_config(SymbolManagerConfig::default());
//! let symbol_map = symbol_manager
//!     .load_symbol_map_for_binary_at_path(Path::new("/usr/bin/ls"), None)
//!     .await?;
//! println!("Looking up 0xd6f4 in /usr/bin/ls. Results:");
//! if let Some(address_info) = symbol_map.lookup(LookupAddress::Relative(0xd6f4)).await {
//!     println!(
//!         "Symbol: {:#x} {}",
//!         address_info.symbol.address, address_info.symbol.name
//!     );
//!     if let Some(frames) = address_info.frames {
//!         for (i, frame) in frames.into_iter().enumerate() {
//!             let function = frame.function.unwrap();
//!             let file = frame.file_path.unwrap().display_path();
//!             let line = frame.line_number.unwrap();
//!             println!("  #{i:02} {function} at {file}:{line}");
//!         }
//!     }
//! } else {
//!     println!("No symbol for 0xd6f4 was found.");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! This example prints the following output on my machine:
//!
//! ```plain
//! Looking up 0xd6f4 in /usr/bin/ls. Results:
//! Symbol: 0xd5d4 gobble_file.constprop.0
//!   #00 do_lstat at ./src/ls.c:1184
//!   #01 gobble_file at ./src/ls.c:3403
//! ```
//!
//! The example demonstrates support for `debuglink` and `debugaltlink`. It gets the symbol
//! information from local debug files at
//! `/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug`
//! and at `/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug`, which were installed
//! by the `coreutils-dbgsym` package. If these files are not present, it will fall back
//! to whichever information is available.
//!
//! # Features
//!
//! ## Windows
//!
//! Supported symbol file sources:
//!
//!  - [x] Local PDB files at the absolute PDB path that's written down in the .exe / .dll
//!  - [x] PDB files on Windows symbol servers + `_NT_SYMBOL_PATH` environment variable
//!  - [x] Breakpad symbol files, local or on a server
//!  - [x] DWARF-in-PE debug info
//!  - [x] Fallback symbols from exported functions and function start addresses
//!
//! Unsupported for now (patches accepted):
//!
//!  - [ ] Support for `/DEBUG:FASTLINK` PDB files ([issue #53](https://github.com/mstange/pdb-addr2line/issues/53))
//!
//! ## macOS
//!
//! Supported symbol file sources:
//!
//!  - [x] Local dSYM bundles with symbol tables + DWARF, found in vicinity of the binary
//!  - [x] dSYM bundles found via Spotlight
//!  - [x] DWARF found in object files which are referred to from a linked binary (via OSO stabs symbols)
//!  - [x] Breakpad symbol files, local or on a server
//!  - [x] Symbols from the regular symbol table
//!  - [x] Fallback symbols from exported functions and function start addresses
//!
//! Unsupported for now (patches accepted):
//!
//!  - [ ] [Finding dSYMs via `DBGFileMappedPaths`, `DBGShellCommands` or `DBGSpotlightPaths`
//!    system settings](https://lldb.llvm.org/use/symbols.html)
//!
//! ## Linux
//!
//! Supported symbol file sources:
//!
//!  - [x] DWARF and symbol tables in binaries
//!  - [x] DWARF and symbol tables in [separate debug files](https://sourceware.org/gdb/onlinedocs/gdb/Separate-Debug-Files.html) found via build ID or debug link
//!  - [x] Symbol tables in [MiniDebugInfo](https://sourceware.org/gdb/onlinedocs/gdb/MiniDebugInfo.html)
//!  - [x] Combining multiple files with DWARF if debug info has been partially moved with `dwz` (using `debugaltlink`)
//!  - [x] [debuginfod](https://sourceware.org/elfutils/Debuginfod.html) servers and the `DEBUGINFOD_URLS` environment variable
//!  - [x] Breakpad symbol files, local or on a server
//!  - [x] Symbols from the regular symbol table
//!  - [x] Fallback symbols from exported functions and function start addresses
//!  - [x] Split DWARF with .dwo files
//!  - [x] Split DWARF with .dwp files
//!
//! # Performance
//!
//! The most computationally intense part of symbol resolution is the parsing of debug info.
//! Debug info can be very large, for example 700MB to 1500MB for Firefox's libxul.
//! `wholesym` uses the [`addr2line`](https://docs.rs/addr2line) and [`pdb-addr2line`](https://docs.rs/pdb-addr2line)
//! crates for parsing DWARF and PDB, respectively. It also has its own code for parsing the
//! Breakpad sym format. All of these parsers have been optimized extensively
//! to minimize the time it takes to get the first symbol result, and to cache
//! things so that repeated lookups in the same functions are fast. This means:
//!
//!  - No expensive preprocessing happens when the symbol file is first loaded.
//!  - Parsing is as lazy as possible: If possible, we only parse the bytes that
//!    are needed for the function which contains the looked-up address.
//!  - The first parse is as shallow and fast as possible, and just builds an index.
//!  - Strings (e.g. function names and file paths) are only looked up when needed.
//!  - Symbol lists, line records, and inlines are cached in sorted structures,
//!    and queried via binary search.

pub use debugid;

mod breakpad;
mod config;
mod debuginfod;
mod download;
mod file_creation;
mod helper;
mod moria_mac;
#[cfg(target_os = "macos")]
mod moria_mac_spotlight;
mod symbol_manager;
mod vdso;

pub use config::SymbolManagerConfig;
pub use samply_symbols;
pub use samply_symbols::{
    AddressInfo, CodeId, ElfBuildId, Error, ExternalFileAddressInFileRef, ExternalFileAddressRef,
    ExternalFileRef, ExternalFileSymbolMap, FrameDebugInfo, FramesLookupResult, LibraryInfo,
    LookupAddress, MappedPath, MultiArchDisambiguator, PeCodeId, SourceFilePath, SymbolInfo,
    SyncAddressInfo,
};
pub use symbol_manager::{SymbolFileOrigin, SymbolManager, SymbolMap};
