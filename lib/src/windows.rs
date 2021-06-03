use crate::error::{Context, GetSymbolsError, Result};
use crate::shared::{
    object_to_map, AddressDebugInfo, FileAndPathHelper, FileContents, FileContentsWrapper,
    InlineStackFrame, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use pdb::{FallibleIterator, PublicSymbol, SymbolData, PDB};
use pdb_addr2line::{TypeFormatter, TypeFormatterFlags};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::{borrow::Cow, path::Path};

pub async fn get_symbolication_result_via_binary<R>(
    file_kind: object::FileKind,
    file_contents: FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery<'_>,
    path: &Path,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let SymbolicationQuery {
        debug_name,
        breakpad_id,
        addresses,
        ..
    } = query.clone();
    use object::Object;
    let pe = object::File::parse(&file_contents)
        .map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;
    let info = match pe.pdb_info() {
        Ok(Some(info)) => info,
        _ => {
            return Err(GetSymbolsError::NoDebugInfoInPeBinary(
                path.to_string_lossy().to_string(),
            ))
        }
    };

    // We could check the binary's signature here against breakpad_id, but we don't really
    // care whether we have the right binary. As long as we find a PDB file with the right
    // signature, that's all we need, and we'll happily accept correct PDB files even when
    // we found them via incorrect binaries.

    let pdb_path =
        std::ffi::CString::new(info.path()).expect("info.path() should have stripped the nul byte");

    let candidate_paths_for_pdb = helper
        .get_candidate_paths_for_pdb(debug_name, breakpad_id, &pdb_path, path)
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
        if let Ok(table) =
            try_get_symbolication_result_from_pdb_path(query.clone(), &pdb_path, helper).await
        {
            return Ok(table);
        }
    }

    // Fallback: If no PDB file is present, make a symbol table with just the exports.
    // Now it's time to check the breakpad ID!

    let signature = pe_signature_to_uuid(&info.guid());
    let expected_breakpad_id = format!("{:X}{:x}", signature.to_simple(), info.age());

    if breakpad_id != expected_breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            expected_breakpad_id,
            breakpad_id.to_string(),
        ));
    }

    let mut map = object_to_map(&pe);
    if let Ok(exports) = pe.exports() {
        let image_base_address: u64 = pe.relative_address_base();
        for export in exports {
            if let Ok(name) = std::str::from_utf8(export.name()) {
                map.insert((export.address() - image_base_address) as u32, name);
            }
        }
    }
    Ok(R::from_full_map(map, addresses))
}

async fn try_get_symbolication_result_from_pdb_path<R>(
    query: SymbolicationQuery<'_>,
    path: &Path,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let file_contents = FileContentsWrapper::new(helper.open_file(path).await.map_err(|e| {
        GetSymbolsError::HelperErrorDuringOpenFile(path.to_string_lossy().to_string(), e)
    })?);
    let buffer = file_contents.read_entire_data().map_err(|e| {
        GetSymbolsError::HelperErrorDuringFileReading(path.to_string_lossy().to_string(), e)
    })?;
    let pdb_reader = Cursor::new(buffer);
    let pdb = PDB::open(pdb_reader)?;
    get_symbolication_result(pdb, query)
}

pub fn get_symbolication_result<'a, 's, S, R>(
    mut pdb: PDB<'s, S>,
    query: SymbolicationQuery<'a>,
) -> Result<R>
where
    R: SymbolicationResult,
    S: pdb::Source<'s> + 's,
{
    // Check against the expected breakpad_id.
    let info = pdb.pdb_information().context("pdb_information")?;
    let dbi = pdb.debug_information()?;
    let age = dbi.age().unwrap_or(info.age);
    let pdb_id = format!("{:X}{:x}", info.guid.to_simple(), age);

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
    let mut symbol_map: BTreeMap<_, _> = global_symbols
        .iter()
        .filter_map(|symbol| {
            Ok(match symbol.parse() {
                Ok(SymbolData::Public(PublicSymbol {
                    function: true,
                    name,
                    offset,
                    ..
                })) => {
                    if let Some(rva) = offset.to_rva(&addr_map) {
                        Some((rva.0, name.to_string()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
        })
        .collect()?;

    // Add Procedure symbols from the modules.
    let tpi = pdb.type_information().context("type_information")?;
    let ipi = pdb.id_information().context("id_information")?;
    let flags = TypeFormatterFlags::default() | TypeFormatterFlags::NO_MEMBER_FUNCTION_STATIC;
    let type_formatter = TypeFormatter::new(&dbi, &tpi, &ipi, flags)?;
    let context_data = pdb_addr2line::ContextConstructionData::try_from_pdb(&mut pdb)
        .context("ContextConstructionData::try_from_pdb")?;
    let context =
        pdb_addr2line::Context::new(&context_data, &type_formatter).context("Context::new")?;

    match R::result_kind() {
        SymbolicationResultKind::AllSymbols => {
            for procedure in context.iter_procedures() {
                let symbol_address = procedure.procedure_start_rva;
                let symbol_name = match procedure.function {
                    Some(name) => name,
                    None => "unknown".to_string(),
                };
                symbol_map
                    .entry(symbol_address)
                    .or_insert_with(|| Cow::from(symbol_name));
            }
            let symbolication_result = R::from_full_map(symbol_map, addresses);
            Ok(symbolication_result)
        }
        SymbolicationResultKind::SymbolsForAddresses { with_debug_info } => {
            let mut symbolication_result = R::for_addresses(addresses);
            for &address in addresses {
                if with_debug_info {
                    if let Some(procedure_frames) = context.find_frames(address)? {
                        let symbol_address = procedure_frames.procedure_start_rva;
                        let symbol_name = match &procedure_frames.frames.last().unwrap().function {
                            Some(name) => name,
                            None => "unknown",
                        };
                        symbolication_result.add_address_symbol(
                            address,
                            symbol_address,
                            symbol_name,
                        );
                        let frames: Vec<_> = procedure_frames
                            .frames
                            .into_iter()
                            .map(convert_stack_frame)
                            .collect();
                        symbolication_result
                            .add_address_debug_info(address, AddressDebugInfo { frames });
                    }
                } else if let Some(procedure) = context.find_function(address)? {
                    let symbol_address = procedure.procedure_start_rva;
                    let symbol_name = match &procedure.function {
                        Some(name) => name,
                        None => "unknown",
                    };
                    symbolication_result.add_address_symbol(address, symbol_address, symbol_name);
                }
            }
            let total_symbol_count = symbol_map.len() + context.procedure_count();
            symbolication_result.set_total_symbol_count(total_symbol_count as u32);
            Ok(symbolication_result)
        }
    }
}

fn convert_stack_frame(frame: pdb_addr2line::Frame<'_>) -> InlineStackFrame {
    InlineStackFrame {
        function: frame.function,
        file_path: frame.file.map(|s| s.to_string()),
        line_number: frame.line,
    }
}

fn pe_signature_to_uuid(identifier: &[u8; 16]) -> uuid::Uuid {
    let mut data = *identifier;
    // The PE file targets a little endian architecture. Convert to
    // network byte order (big endian) to match the Breakpad processor's
    // expectations. For big endian object files, this is not needed.
    data[0..4].reverse(); // uuid field 1
    data[4..6].reverse(); // uuid field 2
    data[6..8].reverse(); // uuid field 3

    uuid::Uuid::from_bytes(data)
}

#[derive(Clone)]
struct ReadView {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for ReadView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReadView({} bytes)", self.bytes.len())
    }
}

impl pdb::SourceView<'_> for ReadView {
    fn as_slice(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

impl<'s, F: FileContents> pdb::Source<'s> for &'s FileContentsWrapper<F> {
    fn view(
        &mut self,
        slices: &[pdb::SourceSlice],
    ) -> std::result::Result<Box<dyn pdb::SourceView<'s>>, std::io::Error> {
        let len = slices.iter().fold(0, |acc, s| acc + s.size);

        let mut v = ReadView {
            bytes: Vec::with_capacity(len),
        };
        v.bytes.resize(len, 0);

        {
            let bytes = v.bytes.as_mut_slice();
            let mut output_offset: usize = 0;
            for slice in slices {
                let slice_buf = self
                    .read_bytes_at(slice.offset, slice.size as u64)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                bytes[output_offset..(output_offset + slice.size)].copy_from_slice(slice_buf);
                output_offset += slice.size;
            }
        }

        Ok(Box::new(v))
    }
}
