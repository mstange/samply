use crate::debugid_util::debug_id_for_object;
use crate::dwarf::{collect_dwarf_address_debug_data, make_address_pairs_for_root_object};
use crate::error::{GetSymbolsError, Result};
use crate::path_mapper::PathMapper;
use crate::shared::{
    get_symbolication_result_for_addresses_from_object, object_to_map, BasePath, FileContents,
    FileContentsWrapper, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use gimli::{CieOrFde, EhFrame, UnwindSection};
use object::{File, FileKind, Object, ObjectSection, ReadRef};
use std::io::Cursor;

pub fn get_symbolication_result<R>(
    base_path: &BasePath,
    file_kind: FileKind,
    file_contents: FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let elf_file =
        File::parse(&file_contents).map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;

    let elf_debug_id = debug_id_for_object(&elf_file).ok_or(GetSymbolsError::InvalidInputError(
        "debug ID cannot be read",
    ))?;
    let SymbolicationQuery { debug_id, .. } = query;
    if elf_debug_id != debug_id {
        return Err(GetSymbolsError::UnmatchedDebugId(elf_debug_id, debug_id));
    }

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(debugdata) = elf_file.section_by_name(".gnu_debugdata") {
        if let Ok(data) = debugdata.data() {
            let mut cursor = Cursor::new(data);
            let mut objdata = Vec::new();
            if let Ok(()) = lzma_rs::xz_decompress(&mut cursor, &mut objdata) {
                if let Ok(elf_file) = File::parse(&objdata[..]) {
                    let file_contents = FileContentsWrapper::new(&objdata[..]);
                    return get_symbolication_result_impl(
                        elf_file,
                        base_path,
                        &file_contents,
                        query,
                    );
                }
            }
        }
    }

    get_symbolication_result_impl(elf_file, base_path, &file_contents, query)
}

pub fn get_symbolication_result_impl<'data, R>(
    elf_file: File<'data, impl ReadRef<'data>>,
    base_path: &BasePath,
    file_contents: &'data FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let (function_starts, function_ends) = function_start_and_end_addresses(&elf_file);
    let (addresses, mut symbolication_result) = match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            let map = object_to_map(&elf_file, Some(&function_starts));
            return Ok(R::from_full_map(map));
        }
        SymbolicationResultKind::SymbolsForAddresses {
            addresses,
            with_debug_info,
        } => {
            let symbolication_result = get_symbolication_result_for_addresses_from_object(
                addresses,
                &elf_file,
                Some(&function_starts),
                Some(&function_ends),
            );
            if !with_debug_info {
                return Ok(symbolication_result);
            }
            (addresses, symbolication_result)
        }
    };

    let addresses: Vec<_> = make_address_pairs_for_root_object(addresses, &elf_file);
    let mut path_mapper = PathMapper::new(base_path);
    collect_dwarf_address_debug_data(
        file_contents.full_range(),
        &elf_file,
        &addresses,
        &mut symbolication_result,
        &mut path_mapper,
    );
    Ok(symbolication_result)
}

/// Get a list of function addresses as u32 relative addresses.
pub fn function_start_and_end_addresses<'a: 'b, 'b, T>(object_file: &'b T) -> (Vec<u32>, Vec<u32>)
where
    T: object::Object<'a, 'b>,
{
    // Get an approximation of the list of function start addresses by
    // iterating over the exception handling info. Every FDE roughly
    // maps to one function.
    // This currently only covers the ELF format. For mach-O, this information is
    // not in .eh_frame, it is in __unwind_info (plus some auxiliary data
    // in __eh_frame, but that's only needed for the actual unwinding, not
    // for the function start addresses).
    // We also don't handle .debug_frame yet, which is sometimes found
    // instead of .eh_frame.
    // And we don't have anything for the PE format yet, either.

    let eh_frame = object_file.section_by_name(".eh_frame");
    let eh_frame_hdr = object_file.section_by_name(".eh_frame_hdr");
    let text = object_file.section_by_name(".text");
    let got = object_file.section_by_name(".got");

    fn section_addr_or_zero<'a>(section: &Option<impl ObjectSection<'a>>) -> u64 {
        match section {
            Some(section) => section.address(),
            None => 0,
        }
    }

    let bases = gimli::BaseAddresses::default()
        .set_eh_frame_hdr(section_addr_or_zero(&eh_frame_hdr))
        .set_eh_frame(section_addr_or_zero(&eh_frame))
        .set_text(section_addr_or_zero(&text))
        .set_got(section_addr_or_zero(&got));

    let endian = if object_file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };

    let address_size = object_file
        .architecture()
        .address_size()
        .unwrap_or(object::AddressSize::U64) as u8;

    let eh_frame = match eh_frame {
        Some(eh_frame) => eh_frame,
        None => return (Vec::new(), Vec::new()),
    };

    let eh_frame_data = match eh_frame.uncompressed_data() {
        Ok(eh_frame_data) => eh_frame_data,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let mut eh_frame = EhFrame::new(&*eh_frame_data, endian);
    eh_frame.set_address_size(address_size);
    let mut cur_cie = None;
    let mut entries_iter = eh_frame.entries(&bases);
    let mut start_addresses = Vec::new();
    let mut end_addresses = Vec::new();
    while let Ok(Some(entry)) = entries_iter.next() {
        match entry {
            CieOrFde::Cie(cie) => cur_cie = Some(cie),
            CieOrFde::Fde(partial_fde) => {
                if let Ok(fde) = partial_fde.parse(|eh_frame, bases, cie_offset| {
                    if let Some(cie) = &cur_cie {
                        if cie.offset() == cie_offset.0 {
                            return Ok(cie.clone());
                        }
                    }
                    let cie = eh_frame.cie_from_offset(bases, cie_offset);
                    if let Ok(cie) = &cie {
                        cur_cie = Some(cie.clone());
                    }
                    cie
                }) {
                    start_addresses.push(fde.initial_address() as u32);
                    end_addresses.push((fde.initial_address() + fde.len()) as u32);
                }
            }
        }
    }
    (start_addresses, end_addresses)
}
