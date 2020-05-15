use crate::error::{Context, GetSymbolsError, Result};
use crate::pdb_crate::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use crate::shared::{FileAndPathHelper, OwnedFileData, SymbolicationQuery, SymbolicationResult};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

pub async fn get_symbolication_result_via_binary<'a, R>(
    buffer: &[u8],
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let SymbolicationQuery {
        debug_name,
        breakpad_id,
        path,
        addresses,
        ..
    } = query.clone();
    let pe = goblin::pe::PE::parse(buffer)?;
    let debug_info = pe.debug_data.and_then(|d| d.codeview_pdb70_debug_info);
    let info = match debug_info {
        None => {
            return Err(GetSymbolsError::NoDebugInfoInPeBinary(
                path.to_string_lossy().to_string(),
            ))
        }
        Some(info) => info,
    };

    // We could check the binary's signature here against breakpad_id, but we don't really
    // care whether we have the right binary. As long as we find a PDB file with the right
    // signature, that's all we need, and we'll happily accept correct PDB files even when
    // we found them via incorrect binaries.

    let pdb_path = std::ffi::CStr::from_bytes_with_nul(info.filename)
        .map_err(|_| GetSymbolsError::PdbPathDidntEndWithNul(path.to_string_lossy().to_string()))?;

    let candidate_paths_for_pdb = helper
        .get_candidate_paths_for_pdb(debug_name, breakpad_id, pdb_path, path)
        .await
        .map_err(|e| {
            GetSymbolsError::HelperErrorDuringGetCandidatePathsForPdb(
                debug_name.to_string(),
                breakpad_id.to_string(),
                e,
            )
        })?;

    for pdb_path in candidate_paths_for_pdb {
        if pdb_path == path {
            continue;
        }
        if let Ok(table) = try_get_symbolication_result_from_pdb_path(query.clone(), helper).await {
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

async fn try_get_symbolication_result_from_pdb_path<'a, R>(
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let owned_data = helper.read_file(query.path).await.map_err(|e| {
        GetSymbolsError::HelperErrorDuringReadFile(query.path.to_string_lossy().to_string(), e)
    })?;
    let pdb_data = owned_data.get_data();
    let pdb_reader = Cursor::new(pdb_data);
    let pdb = PDB::open(pdb_reader)?;
    get_symbolication_result(pdb, query)
}

pub fn get_symbolication_result<'a, 's, S, R>(
    mut pdb: PDB<'s, S>,
    query: SymbolicationQuery<'a>,
) -> Result<R>
where
    R: SymbolicationResult,
    S: pdb_crate::Source<'s> + 's,
{
    // Check against the expected breakpad_id.
    let info = pdb.pdb_information().context("pdb_information")?;
    let pdb_id = format!("{}{:x}", format!("{:X}", info.guid.to_simple()), info.age);

    let SymbolicationQuery {
        breakpad_id,
        addresses,
        ..
    } = query;
    if pdb_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            pdb_id,
            breakpad_id.to_string(),
        ));
    }

    // Now, gather the symbols into a hashmap.
    let addr_map = pdb.address_map().context("address_map")?;

    // Start with the public function symbols.
    let global_symbols = pdb.global_symbols().context("global_symbols")?;
    let mut hashmap: HashMap<_, _> = global_symbols
        .iter()
        .filter_map(|symbol| match symbol.parse() {
            Ok(SymbolData::PublicSymbol(PublicSymbol {
                function: true,
                offset,
                ..
            })) => Some((offset.to_rva(&addr_map)?.0, symbol.name().ok()?.to_string())),
            _ => None,
        })
        .collect()?;

    // Add Procedure symbols from the modules, if present. Some of these might
    // duplicate public symbols; in that case, don't overwrite the existing
    // symbol name because usually the public symbol version has the full
    // function signature whereas the procedure symbol only has the function
    // name itself.
    if let Ok(dbi) = pdb.debug_information() {
        let mut modules = dbi.modules().context("dbi.modules()")?;
        while let Some(module) = modules.next().context("modules.next()")? {
            let info = match pdb.module_info(&module) {
                Ok(info) => info,
                _ => continue,
            };
            let mut symbols = info.symbols().context("info.symbols()")?;
            while let Ok(Some(symbol)) = symbols.next() {
                let offset = match symbol.parse() {
                    Ok(SymbolData::Procedure(ProcedureSymbol { offset, .. })) => offset,
                    _ => continue,
                };
                if let (Ok(name), Some(query)) = (symbol.name(), offset.to_rva(&addr_map)) {
                    hashmap
                        .entry(query.0)
                        .or_insert_with(|| Cow::from(name.to_string().into_owned()));
                }
            }
        }
    }

    Ok(R::from_map(hashmap, addresses))
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
