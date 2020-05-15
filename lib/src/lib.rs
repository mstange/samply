extern crate pdb as pdb_crate;

use goblin;

mod compact_symbol_table;
mod elf;
mod error;
mod macho;
mod pdb;
mod symbolicate;

use goblin::Hint;
use pdb_crate::PDB;
use serde_json::json;
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub use crate::compact_symbol_table::{CompactSymbolTable, SymbolicationResult};
pub use crate::error::{GetSymbolsError, Result};

pub trait OwnedFileData {
    fn get_data(&self) -> &[u8];
}

pub type FileAndPathHelperError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type FileAndPathHelperResult<T> = std::result::Result<T, FileAndPathHelperError>;

pub trait FileAndPathHelper {
    type FileContents: OwnedFileData;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>>>>;

    fn get_candidate_paths_for_pdb(
        &self,
        _debug_name: &str,
        _breakpad_id: &str,
        pdb_path_as_stored_in_binary: &std::ffi::CStr,
        _binary_path: &Path,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>>>> {
        async fn single_value_path_vec(
            path: std::ffi::CString,
        ) -> FileAndPathHelperResult<Vec<PathBuf>> {
            Ok(vec![path.into_string()?.into()])
        }
        Box::pin(single_value_path_vec(
            pdb_path_as_stored_in_binary.to_owned(),
        ))
    }

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::FileContents>>>>;
}

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

#[derive(Clone)]
pub struct SymbolicationQuery<'a> {
    pub debug_name: &'a str,
    pub breakpad_id: &'a str,
    pub path: &'a Path,
    pub addresses: &'a [u32],
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
    let binary_data = owned_data.get_data();

    let mut reader = Cursor::new(binary_data);
    match goblin::peek(&mut reader)? {
        Hint::Elf(_) => elf::get_symbolication_result(binary_data, query),
        Hint::Mach(_) => macho::get_symbolication_result(binary_data, query),
        Hint::MachFat(_) => macho::get_symbolication_result_multiarch(binary_data, query),
        Hint::PE => pdb::get_symbolication_result_via_binary(binary_data, query, helper).await,
        _ => {
            // Might this be a PDB, then?
            let pdb_reader = Cursor::new(binary_data);
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
