//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//! It maps raw code addresses to symbol strings, and, if available, file name + line number
//! information.
//! The API was designed for the Firefox profiler.
//!
//! The main entry point of this crate is the async `get_symbolication_result` function.
//!
//! # Design constraints
//!
//! This crate operates under the following design constraints:
//!
//!  - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
//!    WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
//!    This setup allows us to download the profiler-get-symbols wasm bundle on demand, rather than shipping
//!    it with Firefox, which would increase the Firefox download size for a piece of functionality
//!    that the vast majority of Firefox users don't need.
//!  - Performance: We want to be able to obtain symbol data from a fresh build of a locally compiled
//!    Firefox instance as quickly as possible, without an expensive preprocessing step. The time between
//!    "finished compilation" and "returned symbol data" should be minimized. This means that symbol
//!    data needs to be obtained directly from the compilation artifacts rather than from, say, a
//!    dSYM bundle or a Breakpad .sym file.
//!  - Must scale to large inputs: This applies to both the size of the API request and the size of the
//!    object files that need to be parsed: The Firefox profiler will supply anywhere between tens of
//!    thousands and hundreds of thousands of different code addresses in a single symbolication request.
//!    Firefox build artifacts such as libxul.so can be multiple gigabytes big, and contain around 300000
//!    function symbols. We want to serve such requests within a few seconds or less.
//!  - "Best effort" basis: If only limited symbol information is available, for example from system
//!    libraries, we want to return whatever limited information we have.
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
//! use samply_symbols::{
//!     FileContents, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture,
//!     CandidatePathInfo, FileLocation, AddressDebugInfo, SymbolicationResult, SymbolicationResultKind, SymbolicationQuery
//! };
//! use samply_symbols::debugid::DebugId;
//!
//! async fn run_query() -> String {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
//!     };
//!     let r: Result<ExampleSymbolicationResult, _> = samply_symbols::get_symbolication_result(
//!            SymbolicationQuery {
//!                debug_name: "firefox.pdb",
//!                debug_id: DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").unwrap(),
//!                result_kind: SymbolicationResultKind::SymbolsForAddresses(
//!                    &[204776, 129423, 244290, 244219]
//!                ),
//!            },
//!            &helper,
//!        ).await;
//!     match r {
//!         Ok(res) => format!("{:?}", res),
//!         Err(err) => format!("Error: {}", err),
//!     }
//! }
//!
//! struct ExampleHelper {
//!     artifact_directory: std::path::PathBuf,
//! }
//!
//! impl<'h> FileAndPathHelper<'h> for ExampleHelper {
//!     type F = Vec<u8>;
//!     type OpenFileFuture =
//!         std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;
//!
//!     fn get_candidate_paths_for_binary_or_pdb(
//!         &self,
//!         debug_name: &str,
//!         _debug_id: &DebugId,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(self.artifact_directory.join(debug_name)))])
//!     }
//!
//!     fn open_file(
//!         &'h self,
//!         location: &FileLocation,
//!     ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
//!         async fn read_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
//!             Ok(std::fs::read(&path)?)
//!         }
//!
//!         let path = match location {
//!             FileLocation::Path(path) => path.clone(),
//!             FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
//!         };
//!         Box::pin(read_file_impl(path.to_path_buf()))
//!     }
//! }
//!
//! #[derive(Debug, Default)]
//! struct ExampleSymbolicationResult {
//!     /// For each address, the symbol name and the (optional) debug info.
//!     map: std::collections::HashMap<u32, (String, Option<AddressDebugInfo>)>,
//! }
//!
//! impl SymbolicationResult for ExampleSymbolicationResult {
//!     fn from_full_map<S>(_map: Vec<(u32, S)>) -> Self
//!     where
//!         S: std::ops::Deref<Target = str>,
//!     {
//!         panic!("Should not be called")
//!     }
//!
//!     fn for_addresses(_addresses: &[u32]) -> Self {
//!         Default::default()
//!     }
//!
//!     fn add_address_symbol(
//!         &mut self,
//!         address: u32,
//!         _symbol_address: u32,
//!         symbol_name: String,
//!         _function_size: Option<u32>,
//!     ) {
//!         self.map.insert(address, (symbol_name, None));
//!     }
//!
//!     fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo) {
//!         if let Some(entry) = self.map.get_mut(&address) {
//!             entry.1 = Some(info);
//!         }
//!     }
//!
//!     fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {
//!         // ignored
//!     }
//! }
//! ```

pub use debugid;
pub use object;
pub use pdb_addr2line::pdb;

use debugid::DebugId;
use object::{macho::FatHeader, read::FileKind};
use pdb::PDB;

mod cache;
mod chunked_read_buffer_manager;
mod compact_symbol_table;
mod debugid_util;
mod demangle;
mod demangle_ocaml;
mod dwarf;
mod elf;
mod error;
mod macho;
mod path_mapper;
mod shared;
mod windows;

pub use crate::cache::{FileByteSource, FileContentsWithChunkedCaching};
pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::error::Error;
use crate::shared::FileContentsWrapper;
pub use crate::shared::{
    AddressDebugInfo, CandidatePathInfo, FileAndPathHelper, FileAndPathHelperError,
    FileAndPathHelperResult, FileContents, FileLocation, FilePath, InlineStackFrame,
    OptionallySendFuture, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
pub use debugid_util::{debug_id_for_object, DebugIdExt};

/// Returns a symbol table in `CompactSymbolTable` format for the requested binary.
/// `FileAndPathHelper` must be implemented by the caller, to provide file access.
pub async fn get_compact_symbol_table<'h>(
    debug_name: &str,
    debug_id: DebugId,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<CompactSymbolTable, Error> {
    get_symbolication_result(
        SymbolicationQuery {
            debug_name,
            debug_id,
            result_kind: SymbolicationResultKind::AllSymbols,
        },
        helper,
    )
    .await
}

/// A generic method which is used in the implementation of both `get_compact_symbol_table`
/// and `query_api`. Allows obtaining symbol data for a given binary. The level of detail
/// is determined by `query.result_kind`: The caller can
/// either get a regular symbol table, or extended information for a set of addresses, if
/// the information is present in the found files. See `SymbolicationResultKind` for
/// more details.
pub async fn get_symbolication_result<'h, R>(
    query: SymbolicationQuery<'_>,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<R, Error>
where
    R: SymbolicationResult,
{
    let candidate_paths_for_binary = helper
        .get_candidate_paths_for_binary_or_pdb(query.debug_name, &query.debug_id)
        .map_err(|e| {
            Error::HelperErrorDuringGetCandidatePathsForBinaryOrPdb(
                query.debug_name.to_string(),
                query.debug_id,
                e,
            )
        })?;

    let mut last_err = None;
    for candidate_info in candidate_paths_for_binary {
        let result = match candidate_info {
            CandidatePathInfo::SingleFile(file_location) => {
                try_get_symbolication_result_from_path(query.clone(), &file_location, helper).await
            }
            CandidatePathInfo::InDyldCache {
                dyld_cache_path,
                dylib_path,
            } => {
                macho::try_get_symbolication_result_from_dyld_shared_cache(
                    query.clone(),
                    &dyld_cache_path,
                    &dylib_path,
                    helper,
                )
                .await
            }
        };

        match result {
            Ok(result) => return Ok(result),
            Err(err) => last_err = Some(err),
        };
    }
    Err(last_err.unwrap_or_else(|| {
        Error::NoCandidatePathForBinary(query.debug_name.to_string(), query.debug_id)
    }))
}

async fn try_get_symbolication_result_from_path<'h, R, H>(
    query: SymbolicationQuery<'_>,
    file_location: &FileLocation,
    helper: &'h H,
) -> Result<R, Error>
where
    R: SymbolicationResult,
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
                elf::get_symbolication_result(&base_path, file_kind, file_contents, query)
            }
            FileKind::MachOFat32 => {
                let arches = FatHeader::parse_arch32(&file_contents)
                    .map_err(|e| Error::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, query.debug_id)?;
                macho::get_symbolication_result(
                    &base_path,
                    file_contents,
                    Some(range),
                    query,
                    helper,
                )
                .await
            }
            FileKind::MachOFat64 => {
                let arches = FatHeader::parse_arch64(&file_contents)
                    .map_err(|e| Error::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, query.debug_id)?;
                macho::get_symbolication_result(
                    &base_path,
                    file_contents,
                    Some(range),
                    query,
                    helper,
                )
                .await
            }
            FileKind::MachO32 | FileKind::MachO64 => {
                macho::get_symbolication_result(&base_path, file_contents, None, query, helper)
                    .await
            }
            FileKind::Pe32 | FileKind::Pe64 => {
                windows::get_symbolication_result_via_binary(
                    file_kind,
                    file_contents,
                    query,
                    file_location,
                    helper,
                )
                .await
            }
            _ => Err(Error::InvalidInputError(
                "Input was Archive, Coff or Wasm format, which are unsupported for now",
            )),
        }
    } else if let Ok(pdb) = PDB::open(&file_contents) {
        // This is a PDB file.
        windows::get_symbolication_result(&base_path, pdb, query)
    } else {
        Err(Error::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
    }
}
