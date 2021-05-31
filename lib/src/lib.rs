//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//! It maps raw code addresses to symbol strings, and, if available, file name + line number
//! information.
//! The API was designed for the Firefox profiler.
//!
//! The main entry point of this crate is the async `query_api` function, which accepts a
//! JSON string with the query input. The JSON API matches the API of the [Mozilla
//! symbolication server ("Tecken")](https://tecken.readthedocs.io/en/latest/symbolication.html).
//! An alternative JSON-free API is available too, but it is not very ergonomic.
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
//! ```
//! use profiler_get_symbols::{FileContents, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture, CandidatePathInfo};
//!
//! async fn run_query() -> String {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
//!     };
//!     profiler_get_symbols::query_api(
//!         "/symbolicate/v5",
//!         r#"{
//!             "memoryMap": [
//!               [
//!                 "firefox.pdb",
//!                 "AA152DEB2D9B76084C4C44205044422E1"
//!               ]
//!             ],
//!             "stacks": [
//!               [
//!                 [0, 204776],
//!                 [0, 129423],
//!                 [0, 244290],
//!                 [0, 244219]
//!               ]
//!             ]
//!           }"#,
//!         &helper,
//!     ).await
//! }
//!
//! struct ExampleHelper {
//!     artifact_directory: std::path::PathBuf,
//! }
//!
//! struct RawFileBytes(Vec<u8>);
//!
//! impl FileAndPathHelper for ExampleHelper {
//!     type F = RawFileBytes;
//!
//!     fn get_candidate_paths_for_binary_or_pdb(
//!         &self,
//!         debug_name: &str,
//!         _breakpad_id: &str,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         Ok(vec![CandidatePathInfo::Normal(self.artifact_directory.join(debug_name))])
//!     }
//!
//!     fn open_file(
//!         &self,
//!         path: &std::path::Path,
//!     ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>>>> {
//!         async fn read_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<RawFileBytes> {
//!             Ok(RawFileBytes(std::fs::read(&path)?))
//!         }
//!
//!         Box::pin(read_file_impl(path.to_path_buf()))
//!     }
//! }
//!
//! impl FileContents for RawFileBytes {
//!     #[inline]
//!     fn len(&self) -> u64 {
//!         self.0.len() as u64
//!     }
//!
//!     #[inline]
//!     fn read_bytes_at<'a>(&'a self, offset: u64, size: u64) -> FileAndPathHelperResult<&'a [u8]> {
//!         Ok(&self.0[offset as usize..][..size as usize])
//!     }
//!
//!     #[inline]
//!     fn read_bytes_at_until<'a>(
//!         &'a self,
//!         offset: u64,
//!         delimiter: u8,
//!     ) -> FileAndPathHelperResult<&'a [u8]> {
//!         let slice_to_end = &self.0[offset as usize..];
//!         if let Some(pos) = slice_to_end.iter().position(|b| *b == delimiter) {
//!             Ok(&slice_to_end[..pos])
//!         } else {
//!             Err(Box::new(std::io::Error::new(
//!                 std::io::ErrorKind::InvalidInput,
//!                 "Delimiter not found in RawFileBytes",
//!             )))
//!         }
//!     }
//! }
//! ```

pub use object;
pub use pdb;

use std::path::Path;

use object::{macho::FatHeader, read::macho::DyldCache, read::FileKind, Endianness};
use pdb::PDB;
use serde_json::json;

mod compact_symbol_table;
mod dwarf;
mod elf;
mod error;
mod macho;
mod shared;
mod symbolicate;
mod windows;

use crate::shared::{SymbolicationQuery, SymbolicationResult};

// Just to hide unused method  warnings. Should be exposed differently.
pub use crate::windows::addr2line as pdb_addr2line;

pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::error::{GetSymbolsError, Result};
use crate::shared::FileContentsWrapper;
pub use crate::shared::{
    CandidatePathInfo, FileAndPathHelper, FileAndPathHelperError, FileAndPathHelperResult,
    FileContents, OptionallySendFuture,
};

/// Returns a symbol table in `CompactSymbolTable` format for the requested binary.
/// `FileAndPathHelper` must be implemented by the caller, to provide file access.
pub async fn get_compact_symbol_table(
    debug_name: &str,
    breakpad_id: &str,
    helper: &impl FileAndPathHelper,
) -> Result<CompactSymbolTable> {
    get_symbolication_result(debug_name, breakpad_id, &[], helper).await
}

/// A generic method which is used in the implementation of both `get_compact_symbol_table`
/// and `query_api`. Allows obtaining symbol data for a given binary. The level of detail
/// is determined by the implementation of the `SymbolicationResult` trait: The caller can
/// either get a regular symbol table, or extended information for a set of addresses, if
/// the information is present in the found files. See the `SymbolicationResult` trait for
/// more details.
pub async fn get_symbolication_result<R>(
    debug_name: &str,
    breakpad_id: &str,
    addresses: &[u32],
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let candidate_paths_for_binary = helper
        .get_candidate_paths_for_binary_or_pdb(debug_name, breakpad_id)
        .map_err(|e| {
            GetSymbolsError::HelperErrorDuringGetCandidatePathsForBinaryOrPdb(
                debug_name.to_string(),
                breakpad_id.to_string(),
                e,
            )
        })?;

    let mut last_err = None;
    for candidate_info in candidate_paths_for_binary {
        let query = SymbolicationQuery {
            debug_name,
            breakpad_id,
            addresses,
        };
        let result = match candidate_info {
            CandidatePathInfo::Normal(path) => {
                try_get_symbolication_result_from_path(query, &path, helper).await
            }
            CandidatePathInfo::InDyldCache {
                dyld_cache_path,
                dylib_path,
            } => {
                try_get_symbolication_result_from_dyld_shared_cache(
                    query,
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
        GetSymbolsError::NoCandidatePathForBinary(debug_name.to_string(), breakpad_id.to_string())
    }))
}

/// This is the main API of this crate.
/// It implements the "Tecken" JSON API, which is also used by the Mozilla symbol server.
/// It's intended to be used as a drop-in "local symbol server" which gathers its data
/// directly from file artifacts produced during compilation (rather than consulting
/// e.g. a database).
/// The caller needs to implement the `FileAndPathHelper` trait to provide file system access.
/// The return value is a JSON string.
///
/// The following "URLs" are supported:
///  - `/symbolicate/v5`: This API is documented at <https://tecken.readthedocs.io/en/latest/symbolication.html>.
///  - `/symbolicate/v6a1`: Same request API as v5, but richer response data. This is still experimental.
///    See the raw response struct definitions in symbolicate/v6/response_json.rs for details.
///    V6 extends V5 by inline callstacks and filename + line number data, and richer error reporting.
pub async fn query_api(
    request_url: &str,
    request_json_data: &str,
    helper: &impl FileAndPathHelper,
) -> String {
    if request_url == "/symbolicate/v5" {
        symbolicate::v5::query_api_json(request_json_data, helper).await
    } else if request_url == "/symbolicate/v6a1" {
        symbolicate::v6::query_api_json(request_json_data, helper).await
    } else {
        json!({ "error": format!("Unrecognized URL {}", request_url) }).to_string()
    }
}

async fn try_get_symbolication_result_from_path<R, H>(
    query: SymbolicationQuery<'_>,
    path: &Path,
    helper: &H,
) -> Result<R>
where
    R: SymbolicationResult,
    H: FileAndPathHelper,
{
    let file_contents = helper.open_file(path).await.map_err(|e| {
        GetSymbolsError::HelperErrorDuringOpenFile(path.to_string_lossy().to_string(), e)
    })?;

    let file_contents = FileContentsWrapper::new(file_contents);

    if let Ok(pdb) = PDB::open(&file_contents) {
        // This is a PDB file.
        return windows::get_symbolication_result(pdb, query);
    }

    if let Ok(file_kind) = FileKind::parse(&file_contents) {
        match file_kind {
            FileKind::Elf32 | FileKind::Elf64 => {
                elf::get_symbolication_result(file_kind, file_contents, query)
            }
            FileKind::MachOFat32 => {
                let arches = FatHeader::parse_arch32(&file_contents)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, query.breakpad_id)?;
                macho::get_symbolication_result(file_contents, Some(range), 0, query, helper).await
            }
            FileKind::MachOFat64 => {
                let arches = FatHeader::parse_arch64(&file_contents)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, query.breakpad_id)?;
                macho::get_symbolication_result(file_contents, Some(range), 0, query, helper).await
            }
            FileKind::MachO32 | FileKind::MachO64 => {
                macho::get_symbolication_result(file_contents, None, 0, query, helper).await
            }
            FileKind::Pe32 | FileKind::Pe64 => {
                windows::get_symbolication_result_via_binary(
                    file_kind,
                    file_contents,
                    query,
                    path,
                    helper,
                )
                .await
            }
            _ => Err(GetSymbolsError::InvalidInputError(
                "Input was Archive, Coff or Wasm format, which are unsupported for now",
            )),
        }
    } else {
        Err(GetSymbolsError::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
    }
}

async fn try_get_symbolication_result_from_dyld_shared_cache<R, H>(
    query: SymbolicationQuery<'_>,
    dyld_cache_path: &Path,
    dylib_path: &str,
    helper: &H,
) -> Result<R>
where
    R: SymbolicationResult,
    H: FileAndPathHelper,
{
    let file_contents = helper.open_file(dyld_cache_path).await.map_err(|e| {
        GetSymbolsError::HelperErrorDuringOpenFile(dyld_cache_path.to_string_lossy().to_string(), e)
    })?;

    let file_contents = FileContentsWrapper::new(file_contents);
    let header_offset = {
        let cache = DyldCache::<Endianness, _>::parse(&file_contents)
            .map_err(GetSymbolsError::DyldCacheParseError)?;
        let image = cache.images().find(|image| image.path() == Ok(dylib_path));
        let image = match image {
            Some(image) => image,
            None => {
                return Err(GetSymbolsError::NoMatchingDyldCacheImagePath(
                    dylib_path.to_string(),
                ))
            }
        };
        image
            .file_offset()
            .map_err(GetSymbolsError::DyldCacheParseError)?
    };

    macho::get_symbolication_result(file_contents, None, header_offset, query, helper).await
}
