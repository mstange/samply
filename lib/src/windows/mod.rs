use crate::error::{Context, GetSymbolsError, Result};
use crate::shared::{
    object_to_map, AddressDebugInfo, FileAndPathHelper, FileContents, FileContentsWrapper,
    InlineStackFrame, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use crate::windows::addr2line::Addr2LineContext;
use object::pe::{ImageDosHeader, ImageNtHeaders32, ImageNtHeaders64};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader};
use object::ReadRef;
use pdb::{FallibleIterator, ProcedureSymbol, PublicSymbol, SymbolData, PDB};
use pdb_addr2line::{TypeFormatter, TypeFormatterFlags};
use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;
use std::{borrow::Cow, path::Path};

mod addr2line;

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
    let is_64 = match file_kind {
        object::FileKind::Pe32 => false,
        object::FileKind::Pe64 => true,
        _ => panic!("Unexpected file_kind"),
    };

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
        let image_base_address: u64 = match is_64 {
            false => get_image_base_address::<ImageNtHeaders32, _>(&file_contents).unwrap_or(0),
            true => get_image_base_address::<ImageNtHeaders64, _>(&file_contents).unwrap_or(0),
        };
        for export in exports {
            if let Ok(name) = std::str::from_utf8(export.name()) {
                map.insert((export.address() - image_base_address) as u32, name);
            }
        }
    }
    Ok(R::from_full_map(map, addresses))
}

fn get_image_base_address<'data, Pe: ImageNtHeaders, R: ReadRef<'data>>(data: R) -> Option<u64> {
    let dos_header = ImageDosHeader::parse(data).ok()?;
    let mut offset = dos_header.nt_headers_offset().into();
    let (nt_headers, _) = Pe::parse(data, &mut offset).ok()?;
    let optional_header = nt_headers.optional_header();
    Some(optional_header.image_base())
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
    let tpi = pdb.type_information()?;
    let ipi = pdb.id_information()?;
    let flags = TypeFormatterFlags::default() | TypeFormatterFlags::NO_MEMBER_FUNCTION_STATIC;
    let type_formatter = TypeFormatter::new(&dbi, &tpi, &ipi, flags)?;
    let string_table = pdb.string_table()?;
    let mut modules = dbi.modules().context("dbi.modules()")?;

    match R::result_kind() {
        SymbolicationResultKind::AllSymbols => {
            while let Some(module) = modules.next().context("modules.next()")? {
                let info = match pdb.module_info(&module) {
                    Ok(Some(info)) => info,
                    _ => continue,
                };
                let mut symbols = info.symbols().context("info.symbols()")?;
                while let Ok(Some(symbol)) = symbols.next() {
                    if let Ok(SymbolData::Procedure(ProcedureSymbol {
                        offset,
                        name,
                        type_index,
                        ..
                    })) = symbol.parse()
                    {
                        if let Some(rva) = offset.to_rva(&addr_map) {
                            let mut formatted_name = String::new();
                            type_formatter.write_function(
                                &mut formatted_name,
                                &name.to_string(),
                                type_index,
                            )?;
                            symbol_map
                                .entry(rva.0)
                                .or_insert_with(|| Cow::from(formatted_name));
                        }
                    }
                }
            }
            let symbolication_result = R::from_full_map(symbol_map, addresses);
            Ok(symbolication_result)
        }
        SymbolicationResultKind::SymbolsForAddresses { with_debug_info } => {
            let addr2line_context = if with_debug_info {
                Addr2LineContext::new(&mut pdb, &addr_map, &string_table, &dbi, &type_formatter)
                    .ok()
            } else {
                None
            };
            let mut symbolication_result = R::for_addresses(addresses);
            let mut all_symbol_addresses: HashSet<u32> = symbol_map.keys().cloned().collect();
            while let Some(module) = modules.next().context("modules.next()")? {
                let info = match pdb.module_info(&module) {
                    Ok(Some(info)) => info,
                    _ => continue,
                };

                let mut symbols = info.symbols().context("info.symbols()")?;
                while let Ok(Some(symbol)) = symbols.next() {
                    if let Ok(SymbolData::Procedure(proc)) = symbol.parse() {
                        let ProcedureSymbol {
                            offset,
                            len,
                            name,
                            type_index,
                            ..
                        } = proc;
                        if let Some(rva) = offset.to_rva(&addr_map) {
                            all_symbol_addresses.insert(rva.0);
                            let rva_range = rva.0..(rva.0 + len);
                            let covered_addresses =
                                get_addresses_covered_by_range(addresses, rva_range.clone());
                            if !covered_addresses.is_empty() {
                                if let Some(context) = &addr2line_context {
                                    for address in covered_addresses.iter().cloned() {
                                        let frames = context.find_frames(address)?;
                                        if let Some(name) = frames.last().unwrap().function.clone()
                                        {
                                            symbolication_result
                                                .add_address_symbol(address, rva.0, &name);
                                        }
                                        let frames: Vec<_> =
                                            frames.into_iter().map(convert_stack_frame).collect();
                                        symbolication_result.add_address_debug_info(
                                            address,
                                            AddressDebugInfo { frames },
                                        );
                                    }
                                } else {
                                    let mut formatted_name = String::new();
                                    type_formatter.write_function(
                                        &mut formatted_name,
                                        &name.to_string(),
                                        type_index,
                                    )?;
                                    for address in covered_addresses {
                                        symbolication_result.add_address_symbol(
                                            *address,
                                            rva.0,
                                            &formatted_name,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let total_symbol_count = all_symbol_addresses.len() as u32;
            symbolication_result.set_total_symbol_count(total_symbol_count);
            Ok(symbolication_result)
        }
    }
}

pub fn get_addresses_covered_by_range(addresses: &[u32], range: std::ops::Range<u32>) -> &[u32] {
    let start_index = match addresses.binary_search(&range.start) {
        Ok(i) => i,
        Err(i) => i,
    };
    let half_range = &addresses[start_index..];
    let len = match half_range.binary_search(&range.end) {
        Ok(i) => i,
        Err(i) => i,
    };
    &half_range[..len]
}

fn convert_stack_frame(frame: addr2line::Frame<'_>) -> InlineStackFrame {
    let mut file_path = None;
    let mut line_number = None;
    if let Some(location) = frame.location {
        if let Some(file) = location.file {
            file_path = Some(file.to_string());
        }
        line_number = location.line;
    }
    InlineStackFrame {
        function: frame.function,
        file_path,
        line_number,
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

#[cfg(test)]
mod test {
    use super::get_addresses_covered_by_range;
    #[test]
    fn test_get_addresses_covered_by_range() {
        let empty_slice: &[u32] = &[];
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 0..1),
            empty_slice
        );
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 0..2),
            empty_slice
        );
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 0..3), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 2..3), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 2..4), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 2..6), &[2, 4]);
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 3..4),
            empty_slice
        );
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 2..7), &[2, 4, 6]);
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 5..5),
            empty_slice
        );
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 6..6),
            empty_slice
        );
        assert_eq!(get_addresses_covered_by_range(&[2, 4, 6], 6..8), &[6]);
        assert_eq!(
            get_addresses_covered_by_range(&[2, 4, 6], 7..8),
            empty_slice
        );
        assert_eq!(get_addresses_covered_by_range(&[2], 0..1), empty_slice);
        assert_eq!(get_addresses_covered_by_range(&[2], 0..2), empty_slice);
        assert_eq!(get_addresses_covered_by_range(&[2], 0..3), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2], 1..3), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2], 2..3), &[2]);
        assert_eq!(get_addresses_covered_by_range(&[2], 3..3), empty_slice);
        assert_eq!(get_addresses_covered_by_range(&[2], 3..4), empty_slice);
    }
}
