use std::io::Cursor;

use debugid::DebugId;
use elsa::sync::FrozenVec;
use gimli::{CieOrFde, Dwarf, EhFrame, EndianSlice, RunTimeEndian, UnwindSection};
use object::{File, FileKind, Object, ObjectSection};
use samply_debugid::ElfBuildId;
use samply_object::{debug_id_for_object, relative_address_base};
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::dwarf::Addr2lineContextData;
use crate::error::Error;
use crate::shared::{FileContents, FileContentsWrapper, FileTypes};
use crate::symbol_map::SymbolMap;
use crate::symbol_map_object::{
    DwoDwarfMaker, ObjectSymbolMap, ObjectSymbolMapInnerWrapper, ObjectSymbolMapOuter,
};

/// Information extracted from the primary ELF file's headers, sufficient to
/// drive the rest of the load without keeping the parsed object alive.
#[derive(Debug, Clone)]
pub(crate) struct ElfPrimaryInfo {
    pub debug_id: Option<DebugId>,
    pub debuglink: Option<ElfDebugLinkInfo>,
    pub debugaltlink: Option<ElfDebugAltLinkInfo>,
}

#[derive(Debug, Clone)]
pub(crate) struct ElfDebugLinkInfo {
    pub name: String,
    pub crc: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ElfDebugAltLinkInfo {
    pub path: String,
    pub build_id: ElfBuildId,
}

/// Sniff a primary ELF file once and capture the bits the loader needs later
/// (debug ID, debuglink, debugaltlink).
pub(crate) fn analyze_elf_primary<F: FileContents>(
    file_contents: &FileContentsWrapper<F>,
    file_kind: FileKind,
) -> Result<ElfPrimaryInfo, Error> {
    let elf_file = File::parse(file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;
    let debug_id = debug_id_for_object(&elf_file);
    let debuglink = elf_file
        .gnu_debuglink()
        .ok()
        .flatten()
        .and_then(|(name, crc)| {
            std::str::from_utf8(name).ok().map(|n| ElfDebugLinkInfo {
                name: n.to_owned(),
                crc,
            })
        });
    let debugaltlink = elf_file
        .gnu_debugaltlink()
        .ok()
        .flatten()
        .and_then(|(path, build_id)| {
            std::str::from_utf8(path).ok().map(|p| ElfDebugAltLinkInfo {
                path: p.to_owned(),
                build_id: ElfBuildId(build_id.to_owned()),
            })
        });
    Ok(ElfPrimaryInfo {
        debug_id,
        debuglink,
        debugaltlink,
    })
}

/// Compute the GNU debuglink CRC of an entire `FileContentsWrapper`.
pub(crate) fn debuglink_crc<F: FileContents>(data: &FileContentsWrapper<F>) -> Result<u32, Error> {
    compute_debug_link_crc_of_file_contents(data)
}

/// If the candidate file at `bytes` has the right `build_id`, return it; else None.
pub(crate) fn supplementary_build_id_matches<F: FileContents>(
    bytes: &FileContentsWrapper<F>,
    expected_build_id: &ElfBuildId,
) -> bool {
    let elf_file = match File::parse(bytes) {
        Ok(f) => f,
        Err(_) => return false,
    };
    elf_file.build_id().ok().flatten() == Some(&expected_build_id.0)
}

/// Build a symbol map for the case where we have only the primary ELF (and
/// optionally a DWP). Tries `.gnu_debugdata` mini-debug-info first; otherwise
/// builds a full symbol map with external-file support so dwarf lookups can
/// reach `.dwo` files.
pub(crate) fn build_elf_symbol_map_no_supplementary<H: FileTypes>(
    file_location: H::FL,
    primary: FileContentsWrapper<H::F>,
    dwp: Option<FileContentsWrapper<H::F>>,
    file_kind: FileKind,
) -> Result<SymbolMap<H>, Error> {
    if let Some(symbol_map) =
        try_get_symbol_map_from_mini_debug_info::<H>(&primary, file_kind, &file_location)
    {
        return Ok(symbol_map);
    }
    let owner = ElfSymbolMapDataAndObjects::new(primary, None, dwp, file_kind, None)?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_with_external_file_support(
        file_location,
        Box::new(symbol_map),
    ))
}

/// Build a symbol map from primary + supplementary (debugaltlink) + optional DWP.
pub(crate) fn build_elf_symbol_map_with_supplementary<H: FileTypes>(
    file_location: H::FL,
    primary: FileContentsWrapper<H::F>,
    supplementary: FileContentsWrapper<H::F>,
    dwp: Option<FileContentsWrapper<H::F>>,
    file_kind: FileKind,
) -> Result<SymbolMap<H>, Error> {
    let owner =
        ElfSymbolMapDataAndObjects::new(primary, Some(supplementary), dwp, file_kind, None)?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_with_external_file_support(
        file_location,
        Box::new(symbol_map),
    ))
}

/// Build a symbol map for a debuglink-matched candidate. The candidate's bytes
/// are the primary content, but we override the debug id with that of the
/// original binary the debuglink points away from.
pub(crate) fn build_elf_symbol_map_for_debuglink_match<H: FileTypes>(
    original_file_location: H::FL,
    candidate: FileContentsWrapper<H::F>,
    dwp: Option<FileContentsWrapper<H::F>>,
    file_kind: FileKind,
    debug_id: DebugId,
) -> Result<SymbolMap<H>, Error> {
    let owner = ElfSymbolMapDataAndObjects::new(candidate, None, dwp, file_kind, Some(debug_id))?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_plain(
        original_file_location,
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

fn try_get_symbol_map_from_mini_debug_info<H: FileTypes>(
    primary: &FileContentsWrapper<H::F>,
    file_kind: FileKind,
    debug_file_location: &H::FL,
) -> Option<SymbolMap<H>> {
    let elf_file = File::parse(primary).ok()?;
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

    let base_address = relative_address_base(object_file);
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
                    let Some(start_address) = fde
                        .initial_address()
                        .checked_sub(base_address)
                        .and_then(|address| u32::try_from(address).ok())
                    else {
                        continue;
                    };
                    let Some(end_address) = fde
                        .initial_address()
                        .checked_add(fde.len())
                        .and_then(|address| address.checked_sub(base_address))
                        .and_then(|address| u32::try_from(address).ok())
                    else {
                        continue;
                    };
                    start_addresses.push(start_address);
                    end_addresses.push(end_address);
                }
            }
        }
    }
    (Some(start_addresses), Some(end_addresses))
}
