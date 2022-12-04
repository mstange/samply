use crate::debugid_util::debug_id_for_object;
use crate::dwarf::Addr2lineContextData;
use crate::error::Error;
use crate::path_mapper::PathMapper;
use crate::shared::{
    AddressInfo, BasePath, FileContents, FileContentsWrapper, SymbolMap, SymbolMapTrait,
    SymbolMapTypeErased, SymbolMapTypeErasedOwned,
};
use gimli::{CieOrFde, EhFrame, UnwindSection};
use object::{File, FileKind, Object, ObjectSection, ReadRef};
use std::borrow::Cow;
use std::io::Cursor;
use yoke::{Yoke, Yokeable};

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

    let symbol_map = ElfSymbolMap::parse(file_contents, file_kind, base_path)?;
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
    let symbol_map = ElfSymbolMap::parse(file_contents, file_kind, base_path).ok()?;
    Some(SymbolMapTypeErasedOwned(Box::new(symbol_map)))
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

impl<'data, T: FileContents + 'static> ElfObject<'data, T> {
    pub fn make_symbol_map<'file>(
        &'file self,
        base_path: &BasePath,
    ) -> Result<SymbolMapTypeErased<'file>, Error>
    where
        'data: 'file,
    {
        let (function_starts, function_ends) = function_start_and_end_addresses(&self.object);
        let debug_id = debug_id_for_object(&self.object)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;

        let symbol_map = SymbolMap::new(
            &self.object,
            &self.symbol_map_data.file_data,
            debug_id,
            PathMapper::new(base_path),
            Some(&function_starts),
            Some(&function_ends),
            &self.symbol_map_data.addr2line_context_data,
        );
        let symbol_map = SymbolMapTypeErased(Box::new(symbol_map));
        Ok(symbol_map)
    }
}

struct ElfSymbolMapDataWithElfObject<T: FileContents + 'static>(
    Yoke<ElfObject<'static, T>, Box<ElfSymbolMapData<T>>>,
);

pub struct ElfSymbolMap<T: FileContents + 'static>(
    Yoke<SymbolMapTypeErased<'static>, Box<ElfSymbolMapDataWithElfObject<T>>>,
);

impl<T: FileContents + 'static> ElfSymbolMap<T> {
    pub fn parse(
        file_contents: FileContentsWrapper<T>,
        file_kind: FileKind,
        base_path: &BasePath,
    ) -> Result<Self, Error> {
        let owner = ElfSymbolMapData::new(file_contents);
        let owner_with_elf_object = ElfSymbolMapDataWithElfObject(
            Yoke::<ElfObject<T>, _>::try_attach_to_cart(Box::new(owner), |owner| {
                owner.make_elf_object(file_kind)
            })?,
        );
        let owner_with_symbol_map = Yoke::<SymbolMapTypeErased, _>::try_attach_to_cart(
            Box::new(owner_with_elf_object),
            |owner_with_elf_object| {
                let elf_object = owner_with_elf_object.0.get();
                elf_object.make_symbol_map(base_path)
            },
        )?;
        Ok(ElfSymbolMap(owner_with_symbol_map))
    }
}

impl<T: FileContents + 'static> SymbolMapTrait for ElfSymbolMap<T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.0.get().debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.0.get().symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.get().iter_symbols()
    }

    fn to_map(&self) -> Vec<(u32, String)> {
        self.0.get().to_map()
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.get().lookup(address)
    }
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
