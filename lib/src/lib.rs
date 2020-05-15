extern crate pdb as pdb_crate;

use goblin;

mod compact_symbol_table;
mod elf;
mod error;
mod macho;
mod pdb;
mod symbolicate;

use goblin::{mach, Hint};
use pdb_crate::PDB;
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
        .await?;

    let mut last_err = GetSymbolsError::NoCandidatePathForBinary;
    for path in candidate_paths_for_binary {
        match try_get_symbolication_result_from_path(
            debug_name,
            breakpad_id,
            &path,
            addresses,
            helper,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(err) => last_err = err,
        };
    }
    Err(last_err)
}

pub async fn query_api(
    request_url: &str,
    request_json_data: &str,
    helper: &impl FileAndPathHelper,
) -> String {
    assert_eq!(request_url, "/symbolicate/v5");
    symbolicate::v5::query_api_json(request_json_data, helper).await
}

async fn try_get_symbolication_result_from_path<R>(
    debug_name: &str,
    breakpad_id: &str,
    path: &Path,
    addresses: &[u32],
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let owned_data = helper.read_file(&path).await?;
    let binary_data = owned_data.get_data();

    let mut reader = Cursor::new(binary_data);
    match goblin::peek(&mut reader)? {
        Hint::Elf(_) => elf::get_symbolication_result(binary_data, breakpad_id, addresses),
        Hint::Mach(_) => macho::get_symbolication_result(binary_data, breakpad_id, addresses),
        Hint::MachFat(_) => {
            let mut errors = vec![];
            let multi_arch = mach::MultiArch::new(binary_data)?;
            for fat_arch in multi_arch.iter_arches().filter_map(std::result::Result::ok) {
                let arch_slice = fat_arch.slice(binary_data);
                match macho::get_symbolication_result(arch_slice, breakpad_id, addresses) {
                    Ok(table) => return Ok(table),
                    Err(err) => errors.push(err),
                }
            }
            Err(GetSymbolsError::NoMatchMultiArch(errors))
        }
        Hint::PE => {
            let pe = goblin::pe::PE::parse(binary_data)?;
            let debug_info = pe.debug_data.and_then(|d| d.codeview_pdb70_debug_info);
            let info = match debug_info {
                None => return Err(GetSymbolsError::NoDebugInfoInPeBinary),
                Some(info) => info,
            };

            // We could check the binary's signature here against breakpad_id, but we don't really
            // care whether we have the right binary. As long as we find a PDB file with the right
            // signature, that's all we need, and we'll happily accept correct PDB files even when
            // we found them via incorrect binaries.

            let pdb_path = std::ffi::CStr::from_bytes_with_nul(info.filename)
                .map_err(|_| GetSymbolsError::PdbPathDidntEndWithNul)?;

            let candidate_paths_for_pdb = helper
                .get_candidate_paths_for_pdb(debug_name, breakpad_id, pdb_path, path)
                .await?;

            for pdb_path in candidate_paths_for_pdb {
                if pdb_path == path {
                    continue;
                }
                if let Ok(table) = try_get_symbolication_result_from_pdb_path(
                    breakpad_id,
                    &pdb_path,
                    addresses,
                    helper,
                )
                .await
                {
                    return Ok(table);
                }
            }

            // Fallback: If no PDB file is present, make a symbol table with just the exports.
            // Now it's time to check the breakpad ID!

            let signature = pe_signature_to_uuid(&info.signature);
            // TODO: Is the + 1 the right thing to do here? The example PDBs I've looked at have
            // a 2 at the end, but info.age in the corresponding exe/dll files is always 1.
            // Should we maybe check just the signature and not the age?
            let expected_breakpad_id = format!("{:X}{:x}", signature.to_simple(), info.age + 1);

            if breakpad_id != expected_breakpad_id {
                return Err(GetSymbolsError::UnmatchedBreakpadId(
                    expected_breakpad_id,
                    breakpad_id.to_string(),
                ));
            }

            get_symbolication_result_from_pe_binary(pe, addresses)
        }
        _ => {
            // Might this be a PDB, then?
            let pdb_reader = Cursor::new(binary_data);
            match PDB::open(pdb_reader) {
                Ok(pdb) => {
                    // This is a PDB file.
                    pdb::get_symbolication_result(pdb, breakpad_id, addresses)
                }
                Err(_) => Err(GetSymbolsError::InvalidInputError(
                    "Neither goblin::peek nor PDB::open were able to read the file",
                )),
            }
        }
    }
}

async fn try_get_symbolication_result_from_pdb_path<R>(
    breakpad_id: &str,
    path: &Path,
    addresses: &[u32],
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let owned_data = helper.read_file(&path).await?;
    let pdb_data = owned_data.get_data();
    let pdb_reader = Cursor::new(pdb_data);
    let pdb = PDB::open(pdb_reader)?;
    pdb::get_symbolication_result(pdb, breakpad_id, addresses)
}

fn get_symbolication_result_from_pe_binary<R>(pe: goblin::pe::PE, addresses: &[u32]) -> Result<R>
where
    R: SymbolicationResult,
{
    Ok(R::from_map(
        pe.exports
            .iter()
            .map(|export| {
                (
                    export.rva as u32,
                    export.name.unwrap_or("<unknown>").to_owned(),
                )
            })
            .collect(),
        addresses,
    ))
}

fn pe_signature_to_uuid(identifier: &[u8; 16]) -> uuid::Uuid {
    let mut data = identifier.clone();
    // The PE file targets a little endian architecture. Convert to
    // network byte order (big endian) to match the Breakpad processor's
    // expectations. For big endian object files, this is not needed.
    data[0..4].reverse(); // uuid field 1
    data[4..6].reverse(); // uuid field 2
    data[6..8].reverse(); // uuid field 3

    uuid::Uuid::from_bytes(data)
}
