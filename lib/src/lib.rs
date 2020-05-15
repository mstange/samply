extern crate pdb as pdb_crate;

use goblin;

mod compact_symbol_table;
mod elf;
mod error;
mod macho;
mod pdb;
mod symbolicate;
mod shared;

use goblin::Hint;
use pdb_crate::PDB;
use serde_json::json;
use std::io::Cursor;

pub use crate::compact_symbol_table::{CompactSymbolTable};
pub use crate::error::{GetSymbolsError, Result};
pub use crate::shared::{OwnedFileData, FileAndPathHelperError, FileAndPathHelperResult, FileAndPathHelper};
use crate::shared::{SymbolicationResult, SymbolicationQuery};

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
    let owned_data = helper.read_file(query.path).await.map_err(|e| {
        GetSymbolsError::HelperErrorDuringReadFile(query.path.to_string_lossy().to_string(), e)
    })?;
    let buffer = owned_data.get_data();

    let mut reader = Cursor::new(buffer);
    match goblin::peek(&mut reader)? {
        Hint::Elf(_) => elf::get_symbolication_result(buffer, query),
        Hint::Mach(_) => macho::get_symbolication_result(buffer, query),
        Hint::MachFat(_) => macho::get_symbolication_result_multiarch(buffer, query),
        Hint::PE => pdb::get_symbolication_result_via_binary(buffer, query, helper).await,
        _ => {
            // Might this be a PDB, then?
            let pdb_reader = Cursor::new(buffer);
            match PDB::open(pdb_reader) {
                Ok(pdb) => {
                    // This is a PDB file.
                    pdb::get_symbolication_result(pdb, query)
                }
                Err(_) => Err(GetSymbolsError::InvalidInputError(
                    "Neither goblin::peek nor PDB::open were able to read the file",
                )),
            }
        }
    }
}
