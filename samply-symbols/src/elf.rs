use std::io::Cursor;
use std::sync::Arc;

use debugid::DebugId;
use elsa::sync::FrozenVec;
use gimli::{CieOrFde, Dwarf, EhFrame, EndianSlice, RunTimeEndian, UnwindSection};
use object::{File, FileKind, Object, ObjectSection, ReadRef};
use samply_debugid::ElfBuildId;
use samply_object::debug_id_for_object;
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::dwarf::Addr2lineContextData;
use crate::error::Error;
use crate::shared::{FileAndPathHelper, FileContents, FileContentsWrapper, FileLocation};
use crate::symbol_map::SymbolMap;
use crate::symbol_map_object::{
    DwoDwarfMaker, ObjectSymbolMap, ObjectSymbolMapInnerWrapper, ObjectSymbolMapOuter,
};

pub async fn load_symbol_map_for_elf<H: FileAndPathHelper>(
    file_location: H::FL,
    file_contents: FileContentsWrapper<H::F>,
    file_kind: FileKind,
    helper: Arc<H>,
) -> Result<SymbolMap<H>, Error> {
    let elf_file =
        File::parse(&file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    if let Some(symbol_map) =
        try_to_get_symbol_map_from_debug_link(&file_location, &elf_file, file_kind, &*helper).await
    {
        return Ok(symbol_map);
    }

    let dwp_file_contents = if let Some(dwp_file_location) = file_location.location_for_dwp() {
        helper
            .load_file(dwp_file_location)
            .await
            .ok()
            .map(FileContentsWrapper::new)
    } else {
        None
    };

    if let Some(supplementary_file) =
        try_to_load_supplementary_file(&file_location, &elf_file, &*helper).await
    {
        let owner = ElfSymbolMapDataAndObjects::new(
            file_contents,
            Some(supplementary_file),
            dwp_file_contents,
            file_kind,
            None,
        )?;
        let symbol_map = ObjectSymbolMap::new(owner)?;
        return Ok(SymbolMap::new_plain(file_location, Box::new(symbol_map)));
    }

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(symbol_map) =
        try_get_symbol_map_from_mini_debug_info(&elf_file, file_kind, &file_location)
    {
        return Ok(symbol_map);
    }

    let owner =
        ElfSymbolMapDataAndObjects::new(file_contents, None, dwp_file_contents, file_kind, None)?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_with_external_file_support(
        file_location,
        Box::new(symbol_map),
        helper,
    ))
}

async fn try_to_get_symbol_map_from_debug_link<'data, H, R>(
    original_file_location: &H::FL,
    elf_file: &File<'data, R>,
    file_kind: FileKind,
    helper: &H,
) -> Option<SymbolMap<H>>
where
    R: ReadRef<'data>,
    H: FileAndPathHelper,
{
    let (name, crc) = elf_file.gnu_debuglink().ok().flatten()?;
    let debug_id = debug_id_for_object(elf_file)?;
    let name = std::str::from_utf8(name).ok()?;
    let candidate_paths = helper
        .get_candidate_paths_for_gnu_debug_link_dest(original_file_location, name)
        .ok()?;

    for candidate_path in candidate_paths {
        let symbol_map = get_symbol_map_for_debug_link_candidate(
            original_file_location,
            &candidate_path,
            debug_id,
            crc,
            file_kind,
            helper,
        )
        .await;
        if let Ok(symbol_map) = symbol_map {
            return Some(symbol_map);
        }
    }

    None
}

async fn get_symbol_map_for_debug_link_candidate<H>(
    original_file_location: &H::FL,
    path: &H::FL,
    debug_id: DebugId,
    expected_crc: u32,
    file_kind: FileKind,
    helper: &H,
) -> Result<SymbolMap<H>, Error>
where
    H: FileAndPathHelper,
{
    let file_contents = helper
        .load_file(path.clone())
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(path.to_string(), e))?;
    let file_contents = FileContentsWrapper::new(file_contents);
    let actual_crc = compute_debug_link_crc_of_file_contents(&file_contents)?;

    if actual_crc != expected_crc {
        return Err(Error::DebugLinkCrcMismatch(actual_crc, expected_crc));
    }

    let dwp_file_contents = if let Some(dwp_file_location) = path.location_for_dwp() {
        helper
            .load_file(dwp_file_location)
            .await
            .ok()
            .map(FileContentsWrapper::new)
    } else {
        None
    };
    let owner = ElfSymbolMapDataAndObjects::new(
        file_contents,
        None,
        dwp_file_contents,
        file_kind,
        Some(debug_id),
    )?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_plain(
        original_file_location.clone(),
        Box::new(symbol_map),
    ))
}

#[test]
fn test_crc() {
    fn gnu_debuglink_crc32(initial: u32, buf: &[u8]) -> u32 {
        let mut hasher = crc32fast::Hasher::new_with_initial(initial);
        hasher.update(buf);
        hasher.finalize()
    }

    assert_eq!(gnu_debuglink_crc32(0, b"Hello, world!\0"), 2608877062);

    // I got this reference value by pasting the code from the GDB docs into
    // godbolt and this below it:
    //
    // #include <iostream>
    //
    // int main() {
    //     const char s[] = "Hello, world!";
    //     unsigned char* buf = (unsigned char*)(s);
    //     unsigned long crc = gnu_debuglink_crc32(0, buf, sizeof(s));
    //     std::cout << crc << std::endl;
    // }
}

/// Hash the entire file but use `read_bytes_into` so that only a small
/// part of the file is required in memory at the same time.
fn compute_debug_link_crc_of_file_contents<T: FileContents>(
    data: &FileContentsWrapper<T>,
) -> Result<u32, Error> {
    let mut hasher = crc32fast::Hasher::new();

    const CHUNK_SIZE: u64 = 1024 * 1024; // 1MB
    let mut buffer = Vec::with_capacity(CHUNK_SIZE as usize);

    let len = data.len();
    let mut offset = 0;
    while offset < len {
        let chunk_len = CHUNK_SIZE.min(len - offset);
        data.read_bytes_into(&mut buffer, offset, chunk_len as usize)
            .map_err(|e| Error::HelperErrorDuringFileReading("DebugLinkForCrc".to_string(), e))?;
        hasher.update(&buffer);
        buffer.clear();
        offset += CHUNK_SIZE;
    }
    Ok(hasher.finalize())
}

async fn try_to_load_supplementary_file<'data, H, F, R>(
    original_file_location: &H::FL,
    elf_file: &File<'data, R>,
    helper: &H,
) -> Option<FileContentsWrapper<F>>
where
    H: FileAndPathHelper<F = F>,
    R: ReadRef<'data>,
    F: FileContents + 'static,
{
    let (path, supplementary_build_id) = {
        let (path, build_id) = elf_file.gnu_debugaltlink().ok().flatten()?;
        let supplementary_build_id = ElfBuildId(build_id.to_owned());
        let path = std::str::from_utf8(path).ok()?.to_string();
        (path, supplementary_build_id)
    };
    let candidate_paths = helper
        .get_candidate_paths_for_supplementary_debug_file(
            original_file_location,
            &path,
            &supplementary_build_id,
        )
        .ok()?;

    for candidate_path in candidate_paths {
        if let Ok(file_contents) = helper.load_file(candidate_path).await {
            let file_contents = FileContentsWrapper::new(file_contents);
            if let Ok(elf_file) = File::parse(&file_contents) {
                if elf_file.build_id().ok().flatten() == Some(&supplementary_build_id.0) {
                    return Some(file_contents);
                }
            }
        }
    }

    None
}

fn try_get_symbol_map_from_mini_debug_info<'data, R: ReadRef<'data>, H: FileAndPathHelper>(
    elf_file: &File<'data, R>,
    file_kind: FileKind,
    debug_file_location: &H::FL,
) -> Option<SymbolMap<H>> {
    let debugdata = elf_file.section_by_name(".gnu_debugdata")?;
    let data = debugdata.data().ok()?;
    let mut cursor = Cursor::new(data);
    let mut objdata = Vec::new();
    lzma_rs::xz_decompress(&mut cursor, &mut objdata).ok()?;
    let file_contents = FileContentsWrapper::new(objdata);
    let owner = ElfSymbolMapDataAndObjects::new(file_contents, None, None, file_kind, None).ok()?;
    let symbol_map = ObjectSymbolMap::new(owner).ok()?;
    Some(SymbolMap::new_plain(
        debug_file_location.clone(),
        Box::new(symbol_map),
    ))
}

struct ElfSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
    supplementary_file_data: Option<FileContentsWrapper<T>>,
    dwp_file_data: Option<FileContentsWrapper<T>>,
    dwo_file_data: FrozenVec<Box<FileContentsWrapper<T>>>,
}

#[derive(Yokeable)]
struct ElfObjectsWrapper<'data, T: FileContents>(Box<dyn ElfObjectsTrait<T> + Send + Sync + 'data>);

trait ElfObjectsTrait<T: FileContents> {
    fn make_inner(&self) -> Result<ObjectSymbolMapInnerWrapper<'_, T>, Error>;
}

struct ElfObjects<'data, T: FileContents> {
    file_data: &'data FileContentsWrapper<T>,
    supplementary_file_data: Option<&'data FileContentsWrapper<T>>,
    dwp_file_data: Option<&'data FileContentsWrapper<T>>,
    dwo_file_data: &'data FrozenVec<Box<FileContentsWrapper<T>>>,
    override_debug_id: Option<DebugId>,
    addr2line_context_data: Addr2lineContextData,
    object: File<'data, &'data FileContentsWrapper<T>>,
    supplementary_object: Option<File<'data, &'data FileContentsWrapper<T>>>,
    dwp_object: Option<File<'data, &'data FileContentsWrapper<T>>>,
}

impl<'data, T: FileContents + 'static> ElfObjects<'data, T> {
    fn add_dwo_file_and_make_object(
        &self,
        dwo_file_data: T,
    ) -> Result<
        (
            &'data FileContentsWrapper<T>,
            File<'data, &'data FileContentsWrapper<T>>,
        ),
        Error,
    > {
        let data = self
            .dwo_file_data
            .push_get(Box::new(FileContentsWrapper::new(dwo_file_data)));
        let obj = File::parse(data).map_err(|e| Error::ObjectParseError(FileKind::Elf64, e))?;
        Ok((data, obj))
    }

    fn make_addr2line_context(
        &self,
    ) -> Result<addr2line::Context<EndianSlice<'_, RunTimeEndian>>, Error> {
        self.addr2line_context_data.make_context(
            self.file_data,
            &self.object,
            self.supplementary_file_data,
            self.supplementary_object.as_ref(),
        )
    }

    fn make_dwp_package(
        &self,
    ) -> Result<Option<addr2line::gimli::DwarfPackage<EndianSlice<'_, RunTimeEndian>>>, Error> {
        self.addr2line_context_data.make_package(
            self.file_data,
            &self.object,
            self.dwp_file_data,
            self.dwp_object.as_ref(),
        )
    }

    fn debug_id_for_object(&self) -> Option<DebugId> {
        debug_id_for_object(&self.object)
    }

    fn function_addresses(&self) -> (Option<Vec<u32>>, Option<Vec<u32>>) {
        compute_function_addresses_elf(&self.object)
    }
}

impl<T: FileContents + 'static> DwoDwarfMaker<T> for ElfObjects<'_, T> {
    fn add_dwo_and_make_dwarf(
        &self,
        dwo_file_data: T,
    ) -> Result<Option<Dwarf<EndianSlice<'_, RunTimeEndian>>>, Error> {
        let (data, obj) = self.add_dwo_file_and_make_object(dwo_file_data)?;
        let dwarf = self.addr2line_context_data.make_dwarf_for_dwo(data, &obj)?;
        Ok(Some(dwarf))
    }
}

impl<T: FileContents + 'static> ElfObjectsTrait<T> for ElfObjects<'_, T> {
    fn make_inner(&self) -> Result<ObjectSymbolMapInnerWrapper<'_, T>, Error> {
        let debug_id = if let Some(debug_id) = self.override_debug_id {
            debug_id
        } else {
            self.debug_id_for_object()
                .ok_or(Error::InvalidInputError("debug ID cannot be read"))?
        };
        let (function_starts, function_ends) = self.function_addresses();

        let inner = ObjectSymbolMapInnerWrapper::new(
            &self.object,
            self.make_addr2line_context().ok(),
            self.make_dwp_package().ok().flatten(),
            debug_id,
            function_starts.as_deref(),
            function_ends.as_deref(),
            self,
        );

        Ok(inner)
    }
}

struct ElfSymbolMapDataAndObjects<T: FileContents + 'static>(
    Yoke<ElfObjectsWrapper<'static, T>, Box<ElfSymbolMapData<T>>>,
);

impl<T: FileContents + 'static> ElfSymbolMapDataAndObjects<T> {
    pub fn new(
        file_data: FileContentsWrapper<T>,
        supplementary_file_data: Option<FileContentsWrapper<T>>,
        dwp_file_data: Option<FileContentsWrapper<T>>,
        file_kind: FileKind,
        override_debug_id: Option<DebugId>,
    ) -> Result<Self, Error> {
        let data = ElfSymbolMapData {
            file_data,
            supplementary_file_data,
            dwp_file_data,
            dwo_file_data: FrozenVec::new(),
        };
        let data_and_objects = Yoke::try_attach_to_cart(
            Box::new(data),
            move |data: &ElfSymbolMapData<T>| -> Result<ElfObjectsWrapper<'_, T>, Error> {
                let object = File::parse(&data.file_data)
                    .map_err(|e| Error::ObjectParseError(file_kind, e))?;
                let supplementary_object = match data.supplementary_file_data.as_ref() {
                    Some(supplementary_file_data) => Some(
                        File::parse(supplementary_file_data)
                            .map_err(|e| Error::ObjectParseError(file_kind, e))?,
                    ),
                    None => None,
                };
                let dwp_object = match data.dwp_file_data.as_ref() {
                    Some(dwp_file_data) => Some(
                        File::parse(dwp_file_data)
                            .map_err(|e| Error::ObjectParseError(file_kind, e))?,
                    ),
                    None => None,
                };
                let elf_objects = ElfObjects {
                    object,
                    supplementary_object,
                    dwp_object,
                    dwo_file_data: &data.dwo_file_data,
                    file_data: &data.file_data,
                    supplementary_file_data: data.supplementary_file_data.as_ref(),
                    dwp_file_data: data.dwp_file_data.as_ref(),
                    override_debug_id,
                    addr2line_context_data: Addr2lineContextData::new(),
                };
                Ok(ElfObjectsWrapper(Box::new(elf_objects)))
            },
        )?;
        Ok(Self(data_and_objects))
    }
}

impl<T: FileContents + 'static> ObjectSymbolMapOuter<T> for ElfSymbolMapDataAndObjects<T> {
    fn make_symbol_map_inner(&self) -> Result<ObjectSymbolMapInnerWrapper<'_, T>, Error> {
        self.0.get().0.make_inner()
    }
}

fn compute_function_addresses_elf<'data, O: object::Object<'data>>(
    object_file: &O,
) -> (Option<Vec<u32>>, Option<Vec<u32>>) {
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
