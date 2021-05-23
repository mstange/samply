extern crate pdb as pdb_crate;

use object::{macho::FatHeader, read::FileKind};

mod compact_symbol_table;
mod dwarf;
mod elf;
mod error;
mod macho;
mod pdb;
mod shared;
mod symbolicate;

use pdb_crate::PDB;
use serde_json::json;

pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::error::{GetSymbolsError, Result};
pub use crate::shared::{
    FileAndPathHelper, FileAndPathHelperError, FileAndPathHelperResult, FileContents,
    OptionallySendFuture,
};
use crate::shared::{SymbolicationQuery, SymbolicationResult};

// Just to hide unused method  warnings. Should be exposed differently.
pub use crate::pdb::addr2line as pdb_addr2line;

use crate::shared::FileContentsWrapper;

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
    for path in candidate_paths_for_binary {
        let query = SymbolicationQuery {
            debug_name,
            breakpad_id,
            path: &path,
            addresses,
        };
        match try_get_symbolication_result_from_path(query, helper).await {
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

async fn try_get_symbolication_result_from_path<'a, R, H>(
    query: SymbolicationQuery<'a>,
    helper: &H,
) -> Result<R>
where
    R: SymbolicationResult,
    H: FileAndPathHelper,
{
    let file_contents =
        FileContentsWrapper::new(helper.open_file(query.path).await.map_err(|e| {
            GetSymbolsError::HelperErrorDuringOpenFile(query.path.to_string_lossy().to_string(), e)
        })?);

    if let Ok(pdb) = PDB::open(&file_contents) {
        // This is a PDB file.
        return pdb::get_symbolication_result(pdb, query);
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
                macho::get_symbolication_result(file_contents, Some(range), query, helper).await
            }
            FileKind::MachOFat64 => {
                let arches = FatHeader::parse_arch64(&file_contents)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                let range = macho::get_arch_range(&file_contents, arches, query.breakpad_id)?;
                macho::get_symbolication_result(file_contents, Some(range), query, helper).await
            }
            FileKind::MachO32 | FileKind::MachO64 => {
                macho::get_symbolication_result(file_contents, None, query, helper).await
            }
            FileKind::Pe32 | FileKind::Pe64 => {
                let buffer = file_contents.read_entire_data().map_err(|e| {
                    GetSymbolsError::HelperErrorDuringFileReading(
                        query.path.to_string_lossy().to_string(),
                        e,
                    )
                })?;
                pdb::get_symbolication_result_via_binary(buffer, query, helper).await
            }
            FileKind::Archive | _ => Err(GetSymbolsError::InvalidInputError(
                "Input was Archive, Coff or Wasm format, which are unsupported for now",
            )),
        }
    } else {
        Err(GetSymbolsError::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
    }
}
