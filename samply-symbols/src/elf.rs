use crate::debugid_util::debug_id_for_object;
use crate::dwarf::Addr2lineContextData;
use crate::error::Error;
use crate::path_mapper::PathMapper;
use crate::shared::{
    BasePath, FileContents, FileContentsWrapper, FramesLookupResult, SymbolMap, SymbolicationQuery,
    SymbolicationResult, SymbolicationResultKind,
};
use crate::AddressDebugInfo;
use gimli::{CieOrFde, EhFrame, UnwindSection};
use object::{File, FileKind, Object, ObjectSection};
use std::io::Cursor;
use yoke::{Yoke, Yokeable};

pub fn get_symbolication_result<R, T>(
    base_path: &BasePath,
    file_kind: FileKind,
    file_contents: FileContentsWrapper<T>,
    query: SymbolicationQuery,
) -> Result<R, Error>
where
    R: SymbolicationResult,
    T: FileContents + 'static,
{
    let elf_file =
        File::parse(&file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    let elf_debug_id = debug_id_for_object(&elf_file)
        .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;
    let SymbolicationQuery { debug_id, .. } = query;
    if elf_debug_id != debug_id {
        return Err(Error::UnmatchedDebugId(elf_debug_id, debug_id));
    }

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(debugdata) = elf_file.section_by_name(".gnu_debugdata") {
        if let Ok(data) = debugdata.data() {
            let mut cursor = Cursor::new(data);
            let mut objdata = Vec::new();
            if let Ok(()) = lzma_rs::xz_decompress(&mut cursor, &mut objdata) {
                if let Ok(res) = get_symbolication_result_impl(
                    base_path,
                    file_kind,
                    FileContentsWrapper::new(objdata),
                    query.clone(),
                ) {
                    return Ok(res);
                }
            }
        }
    }

    get_symbolication_result_impl(base_path, file_kind, file_contents, query)
}

struct ElfSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
    addr2line_context_data: Addr2lineContextData,
}

impl<T: FileContents> ElfSymbolMapData<T> {
    pub fn new(file_data: FileContentsWrapper<T>) -> Self {
        Self {
            file_data,
            addr2line_context_data: Addr2lineContextData::new(),
        }
    }

    pub fn make_elf_object(&self, file_kind: FileKind) -> Result<ElfObject<'_, T>, Error> {
        let elf_file =
            File::parse(&self.file_data).map_err(|e| Error::ObjectParseError(file_kind, e))?;
        Ok(ElfObject {
            object: elf_file,
            symbol_map_data: self,
        })
    }
}

#[derive(Yokeable)]
struct ElfObject<'data, T: FileContents> {
    object: File<'data, &'data FileContentsWrapper<T>>,
    symbol_map_data: &'data ElfSymbolMapData<T>,
}

pub fn get_symbolication_result_impl<R, T>(
    base_path: &BasePath,
    file_kind: FileKind,
    file_contents: FileContentsWrapper<T>,
    query: SymbolicationQuery,
) -> Result<R, Error>
where
    R: SymbolicationResult,
    T: FileContents + 'static,
{
    let owner = ElfSymbolMapData::new(file_contents);
    let owner_with_elf_object =
        Yoke::<ElfObject<T>, _>::try_attach_to_cart(Box::new(owner), |owner| {
            owner.make_elf_object(file_kind)
        })?;
    let elf_object = owner_with_elf_object.get();
    let (function_starts, function_ends) = function_start_and_end_addresses(&elf_object.object);
    let symbol_map = SymbolMap::new(
        &elf_object.object,
        &elf_object.symbol_map_data.file_data,
        PathMapper::new(base_path),
        Some(&function_starts),
        Some(&function_ends),
        &elf_object.symbol_map_data.addr2line_context_data,
    );
    let addresses = match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            return Ok(R::from_full_map(symbol_map.to_map()));
        }
        SymbolicationResultKind::SymbolsForAddresses(addresses) => addresses,
    };

    let mut symbolication_result = R::for_addresses(addresses);
    symbolication_result.set_total_symbol_count(symbol_map.symbol_count() as u32);

    for &address in addresses {
        if let Some(address_info) = symbol_map.lookup(address) {
            symbolication_result.add_address_symbol(
                address,
                address_info.symbol.address,
                address_info.symbol.name,
                address_info.symbol.size,
            );
            if let FramesLookupResult::Available(frames) = address_info.frames {
                symbolication_result.add_address_debug_info(address, AddressDebugInfo { frames });
            }
        }
    }

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

    let mut eh_frame = EhFrame::new(&eh_frame_data, endian);
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
