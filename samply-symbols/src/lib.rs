//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//! It maps raw code addresses to symbol strings, and, if available, file name + line number
//! information.
//! The API was designed for the Firefox profiler.
//!
//! The main entry point of this crate is the async `get_symbol_map` function.
//!
//! # Design constraints
//!
//! This crate operates under the following design constraints:
//!
//!   - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
//!     WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
//!     This setup allows us to download the profiler-get-symbols wasm bundle on demand, rather than shipping
//!     it with Firefox, which would increase the Firefox download size for a piece of functionality
//!     that the vast majority of Firefox users don't need.
//!   - Performance: We want to be able to obtain symbol data from a fresh build of a locally compiled
//!     Firefox instance as quickly as possible, without an expensive preprocessing step. The time between
//!     "finished compilation" and "returned symbol data" should be minimized. This means that symbol
//!     data needs to be obtained directly from the compilation artifacts rather than from, say, a
//!     dSYM bundle or a Breakpad .sym file.
//!   - Must scale to large inputs: This applies to both the size of the API request and the size of the
//!     object files that need to be parsed: The Firefox profiler will supply anywhere between tens of
//!     thousands and hundreds of thousands of different code addresses in a single symbolication request.
//!     Firefox build artifacts such as libxul.so can be multiple gigabytes big, and contain around 300000
//!     function symbols. We want to serve such requests within a few seconds or less.
//!   - "Best effort" basis: If only limited symbol information is available, for example from system
//!     libraries, we want to return whatever limited information we have.
//!
//! The WebAssembly requirement means that this crate cannot contain any direct file access.
//! Instead, all file access is mediated through a `FileAndPathHelper` trait which has to be implemented
//! by the caller. Furthermore, the API request does not carry any absolute file paths, so the resolution
//! to absolute file paths needs to be done by the caller as well.
//!
//! # Supported formats and data
//!
//! This crate supports obtaining symbol data from PE binaries (Windows), PDB files (Windows),
//! mach-o binaries (including fat binaries) (macOS & iOS), and ELF binaries (Linux, Android, etc.).
//! For mach-o files it also supports finding debug information in external objects, by following
//! OSO stabs entries.
//! It supports gathering both basic symbol information (function name strings) as well as information
//! based on debug data, i.e. inline callstacks where each frame has a function name, a file name,
//! and a line number.
//! For debug data we support both DWARF debug data (inside mach-o and ELF binaries) and PDB debug data.
//!
//! # Example
//!
//! ```rust
//! use samply_symbols::debugid::DebugId;
//! use samply_symbols::{
//!     CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
//!     FramesLookupResult, OptionallySendFuture,
//! };
//!
//! async fn run_query() {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci"),
//!     };
//!
//!     let symbol_map = match samply_symbols::get_symbol_map(
//!         "firefox.pdb",
//!         DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").unwrap(),
//!         &helper,
//!     )
//!     .await
//!     {
//!         Ok(symbol_map) => symbol_map,
//!         Err(e) => {
//!             println!("Error while loading the symbol map: {:?}", e);
//!             return;
//!         }
//!     };
//!
//!     // Look up the symbol for an address.
//!     let lookup_result = symbol_map.lookup(0x1f98f);
//!
//!     match lookup_result {
//!         Some(address_info) => {
//!             // Print the symbol name for this address:
//!             println!("0x1f98f: {}", address_info.symbol.name);
//!
//!             // See if we have debug info (file name + line, and inlined frames):
//!             match address_info.frames {
//!                 FramesLookupResult::Available(frames) => {
//!                     println!("Debug info:");
//!                     for frame in frames {
//!                         println!(
//!                             " - {:?} ({:?}:{:?})",
//!                             frame.function, frame.file_path, frame.line_number
//!                         );
//!                     }
//!                 }
//!                 FramesLookupResult::External(ext_file, ext_file_addr) => {
//!                     // Debug info is located in a different file. Load that file.
//!                     if let Ok(external_file) =
//!                         samply_symbols::get_external_file(&helper, &ext_file).await
//!                     {
//!                         if let Some(frames) = external_file.lookup(&ext_file_addr) {
//!                             println!("Debug info:");
//!                             for frame in frames {
//!                                 println!(
//!                                     " - {:?} ({:?}:{:?})",
//!                                     frame.function, frame.file_path, frame.line_number
//!                                 );
//!                             }
//!                         }
//!                     }
//!                 }
//!                 FramesLookupResult::Unavailable => {}
//!             }
//!         }
//!         None => {
//!             println!("No symbol was found for address 0x1f98f.")
//!         }
//!     }
//! }
//!
//! struct ExampleHelper {
//!     artifact_directory: std::path::PathBuf,
//! }
//!
//! impl<'h> FileAndPathHelper<'h> for ExampleHelper {
//!     type F = Vec<u8>;
//!     type OpenFileFuture = std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     >;
//!
//!     fn get_candidate_paths_for_binary_or_pdb(
//!         &self,
//!         debug_name: &str,
//!         _debug_id: &DebugId,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(
//!             self.artifact_directory.join(debug_name),
//!         ))])
//!     }
//!
//!     fn open_file(
//!         &'h self,
//!         location: &FileLocation,
//!     ) -> std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     > {
//!         async fn read_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
//!             Ok(std::fs::read(&path)?)
//!         }
//!
//!         let path = match location {
//!             FileLocation::Path(path) => path.clone(),
//!             FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
//!         };
//!         Box::pin(read_file_impl(path))
//!     }
//! }
//! ```

pub use debugid;
pub use object;
pub use pdb_addr2line::pdb;

use debugid::DebugId;
use object::{macho::FatHeader, read::FileKind};

mod cache;
mod chunked_read_buffer_manager;
mod compact_symbol_table;
mod debugid_util;
mod demangle;
mod demangle_ocaml;
mod dwarf;
mod elf;
mod error;
mod external_file;
mod macho;
mod path_mapper;
mod shared;
mod symbol_map;
mod symbol_map_object;
mod windows;

pub use crate::cache::{FileByteSource, FileContentsWithChunkedCaching};
pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::debugid_util::{debug_id_for_object, DebugIdExt};
pub use crate::error::Error;
pub use crate::external_file::{get_external_file, ExternalFileSymbolMap};
use crate::shared::FileContentsWrapper;
pub use crate::shared::{
    AddressDebugInfo, CandidatePathInfo, FileAndPathHelper, FileAndPathHelperError,
    FileAndPathHelperResult, FileContents, FileLocation, FilePath, FramesLookupResult,
    InlineStackFrame, OptionallySendFuture,
};
pub use crate::symbol_map::SymbolMap;

/// Returns a symbol table in `CompactSymbolTable` format for the requested binary.
/// `FileAndPathHelper` must be implemented by the caller, to provide file access.
pub async fn get_compact_symbol_table<'h>(
    debug_name: &str,
    debug_id: DebugId,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<CompactSymbolTable, Error> {
    let symbol_map = get_symbol_map(debug_name, debug_id, helper).await?;
    Ok(CompactSymbolTable::from_full_map(symbol_map.to_map()))
}

/// Get a symbol map for the given `(debug_name, debug_id)` pair.
///
/// This consults the helper to list candidate files, and then picks a file with the
/// matching debug ID.
pub async fn get_symbol_map<'h>(
    debug_name: &str,
    debug_id: DebugId,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<SymbolMap, Error> {
    let candidate_paths_for_binary = helper
        .get_candidate_paths_for_binary_or_pdb(debug_name, &debug_id)
        .map_err(|e| {
            Error::HelperErrorDuringGetCandidatePathsForBinaryOrPdb(
                debug_name.to_string(),
                debug_id,
                e,
            )
        })?;

    let mut last_err = None;
    for candidate_info in candidate_paths_for_binary {
        let symbol_map = match candidate_info {
            CandidatePathInfo::SingleFile(file_location) => {
                get_symbol_map_from_path(&file_location, debug_id, helper).await
            }
            CandidatePathInfo::InDyldCache {
                dyld_cache_path,
                dylib_path,
            } => macho::get_symbol_map_for_dyld_cache(&dyld_cache_path, &dylib_path, helper).await,
        };

        match symbol_map {
            Ok(symbol_map) if symbol_map.debug_id() == debug_id => return Ok(symbol_map),
            Ok(symbol_map) => {
                last_err = Some(Error::UnmatchedDebugId(symbol_map.debug_id(), debug_id));
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| Error::NoCandidatePathForBinary(debug_name.to_string(), debug_id)))
}

async fn get_symbol_map_from_path<'h, H>(
    file_location: &FileLocation,
    debug_id: DebugId,
    helper: &'h H,
) -> Result<SymbolMap, Error>
where
    H: FileAndPathHelper<'h>,
{
    let file_contents = helper
        .open_file(file_location)
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(file_location.to_string_lossy(), e))?;
    let base_path = file_location.to_base_path();

    let file_contents = FileContentsWrapper::new(file_contents);

    if let Ok(file_kind) = FileKind::parse(&file_contents) {
        match file_kind {
            FileKind::Elf32 | FileKind::Elf64 => {
                elf::get_symbol_map(file_contents, file_kind, &base_path)
            }
            FileKind::MachOFat32 => {
                let arches = FatHeader::parse_arch32(&file_contents)
                    .map_err(|e| Error::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, debug_id)?;
                macho::get_symbol_map_for_fat_archive_member(&base_path, file_contents, range)
            }
            FileKind::MachOFat64 => {
                let arches = FatHeader::parse_arch64(&file_contents)
                    .map_err(|e| Error::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, debug_id)?;
                macho::get_symbol_map_for_fat_archive_member(&base_path, file_contents, range)
            }
            FileKind::MachO32 | FileKind::MachO64 => {
                macho::get_symbol_map(&base_path, file_contents)
            }
            FileKind::Pe32 | FileKind::Pe64 => {
                match windows::get_symbol_map_for_pdb_corresponding_to_binary(
                    file_kind,
                    &file_contents,
                    file_location,
                    helper,
                )
                .await
                {
                    Ok(symbol_map) => Ok(symbol_map),
                    Err(_) => windows::get_symbol_map_for_pe(file_contents, file_kind, &base_path),
                }
            }
            _ => Err(Error::InvalidInputError(
                "Input was Archive, Coff or Wasm format, which are unsupported for now",
            )),
        }
    } else if windows::is_pdb_file(&file_contents) {
        windows::get_symbol_map_for_pdb(file_contents, &base_path)
    } else {
        Err(Error::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
    }
}
