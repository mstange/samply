extern crate pdb as pdb_crate;

use object::{
    macho::FatHeader,
    read::{
        elf::{ElfFile32, ElfFile64},
        macho::{MachOFile32, MachOFile64},
        FileKind,
    },
    Endianness,
};

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
    FileContentsWrapper, OptionallySendFuture,
};
use crate::shared::{SymbolicationQuery, SymbolicationResult};

// Just to hide unused method  warnings. Should be exposed differently.
pub use crate::pdb::addr2line as pdb_addr2line;

pub async fn get_compact_symbol_table(
    debug_name: &str,
    breakpad_id: &str,
    helper: &impl FileAndPathHelper,
) -> Result<CompactSymbolTable> {
    get_symbolication_result(debug_name, breakpad_id, &[], helper).await
}

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
        .await
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

async fn try_get_symbolication_result_from_path<'a, R>(
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let file_contents =
        FileContentsWrapper::new(helper.open_file(query.path).await.map_err(|e| {
            GetSymbolsError::HelperErrorDuringOpenFile(query.path.to_string_lossy().to_string(), e)
        })?);

    if let Ok(pdb) = PDB::open(&file_contents) {
        // This is a PDB file.
        return pdb::get_symbolication_result(pdb, query);
    }

    let buffer = file_contents.read_entire_data().map_err(|e| {
        GetSymbolsError::HelperErrorDuringFileReading(query.path.to_string_lossy().to_string(), e)
    })?;

    if let Ok(file_kind) = FileKind::parse(buffer) {
        match file_kind {
            FileKind::Elf32 => {
                let file = ElfFile32::<Endianness>::parse(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                elf::get_symbolication_result(file, query)
            }
            FileKind::Elf64 => {
                let file = ElfFile64::<Endianness>::parse(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                elf::get_symbolication_result(file, query)
            }
            FileKind::MachOFat32 => {
                let arches = FatHeader::parse_arch32(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                macho::get_symbolication_result_multiarch(&file_contents, arches, query, helper)
                    .await
            }
            FileKind::MachOFat64 => {
                let arches = FatHeader::parse_arch64(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                macho::get_symbolication_result_multiarch(&file_contents, arches, query, helper)
                    .await
            }
            FileKind::MachO32 => {
                let file = MachOFile32::<Endianness>::parse(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                macho::get_symbolication_result(file, query, helper).await
            }
            FileKind::MachO64 => {
                let file = MachOFile64::<Endianness>::parse(buffer)
                    .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
                macho::get_symbolication_result(file, query, helper).await
            }
            FileKind::Pe32 | FileKind::Pe64 => {
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
