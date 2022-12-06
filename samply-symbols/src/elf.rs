use crate::error::Error;
use crate::shared::{BasePath, FileContents, FileContentsWrapper, SymbolMapTypeErasedOwned};
use crate::symbol_map_object::{
    FunctionAddressesComputer, GenericSymbolMap, ObjectData, ObjectWrapperTrait, SymbolDataTrait,
};
use gimli::{CieOrFde, EhFrame, UnwindSection};
use object::{File, FileKind, Object, ObjectSection, ReadRef};
use std::io::Cursor;

pub fn get_symbol_map<T>(
    file_contents: FileContentsWrapper<T>,
    file_kind: FileKind,
    base_path: &BasePath,
) -> Result<SymbolMapTypeErasedOwned, Error>
where
    T: FileContents + 'static,
{
    let elf_file =
        File::parse(&file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(symbol_map) =
        try_get_symbol_map_from_mini_debug_info(&elf_file, file_kind, base_path)
    {
        return Ok(symbol_map);
    }

    let owner = ElfSymbolMapData::new(file_contents, file_kind);
    let symbol_map = GenericSymbolMap::new(owner, base_path)?;
    Ok(SymbolMapTypeErasedOwned(Box::new(symbol_map)))
}

fn try_get_symbol_map_from_mini_debug_info<'data, R: ReadRef<'data>>(
    elf_file: &File<'data, R>,
    file_kind: FileKind,
    base_path: &BasePath,
) -> Option<SymbolMapTypeErasedOwned> {
    let debugdata = elf_file.section_by_name(".gnu_debugdata")?;
    let data = debugdata.data().ok()?;
    let mut cursor = Cursor::new(data);
    let mut objdata = Vec::new();
    lzma_rs::xz_decompress(&mut cursor, &mut objdata).ok()?;
    let file_contents = FileContentsWrapper::new(objdata);
    let owner = ElfSymbolMapData::new(file_contents, file_kind);
    let symbol_map = GenericSymbolMap::new(owner, base_path).ok()?;
    Some(SymbolMapTypeErasedOwned(Box::new(symbol_map)))
}

struct ElfSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
    file_kind: FileKind,
}

impl<T: FileContents> ElfSymbolMapData<T> {
    pub fn new(file_data: FileContentsWrapper<T>, file_kind: FileKind) -> Self {
        Self {
            file_data,
            file_kind,
        }
    }
}

impl<T: FileContents + 'static> SymbolDataTrait for ElfSymbolMapData<T> {
    fn make_object_wrapper(&self) -> Result<Box<dyn ObjectWrapperTrait + '_>, Error> {
        let object =
            File::parse(&self.file_data).map_err(|e| Error::ObjectParseError(self.file_kind, e))?;
        let object = ObjectData::new(object, ElfFunctionAddressesComputer, &self.file_data);

        Ok(Box::new(object))
    }
}

struct ElfFunctionAddressesComputer;

impl<'data> FunctionAddressesComputer<'data> for ElfFunctionAddressesComputer {
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>,
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
            None => return (None, None),
        };

        let eh_frame_data = match eh_frame.uncompressed_data() {
            Ok(eh_frame_data) => eh_frame_data,
            Err(_) => return (None, None),
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
        (Some(start_addresses), Some(end_addresses))
    }
}
