use crate::debugid_util::debug_id_for_object;
use crate::dwarf::{get_frames, SectionDataNoCopy};
use crate::error::Error;
use crate::path_mapper::PathMapper;
use crate::shared::{
    relative_address_base, BasePath, FileAndPathHelper, FileContents, FileContentsWrapper,
    FileLocation, RangeReadRef, SymbolMap, SymbolicationQuery, SymbolicationResult,
    SymbolicationResultKind,
};
use crate::AddressDebugInfo;
use debugid::DebugId;
use macho_unwind_info::UnwindInfo;
use object::macho::{self, LinkeditDataCommand, MachHeader32, MachHeader64};
use object::read::macho::{FatArch, LoadCommandIterator, MachHeader};
use object::read::{archive::ArchiveFile, File, Object, ObjectSection, ObjectSymbol};
use object::{Endianness, ReadRef};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::Path;

/// Returns the (offset, size) in the fat binary file for the object that matches
// breakpad_id, if found.
pub fn get_arch_range(
    file_contents: &FileContentsWrapper<impl FileContents>,
    arches: &[impl FatArch],
    debug_id: DebugId,
) -> Result<(u64, u64), Error> {
    let mut debug_ids = Vec::new();
    let mut errors = Vec::new();

    for fat_arch in arches {
        let range = fat_arch.file_range();
        let (start, size) = range;
        let file =
            File::parse(file_contents.range(start, size)).map_err(Error::MachOHeaderParseError)?;
        match debug_id_for_object(&file) {
            Some(di) => {
                if di == debug_id {
                    return Ok(range);
                }
                debug_ids.push(di);
            }
            None => {
                errors.push(Error::InvalidInputError("Missing mach-O UUID"));
            }
        }
    }
    Err(Error::NoMatchMultiArch(debug_ids, errors))
}

pub async fn try_get_symbolication_result_from_dyld_shared_cache<'h, R, H>(
    query: SymbolicationQuery<'_>,
    dyld_cache_path: &Path,
    dylib_path: &str,
    helper: &'h H,
) -> Result<R, Error>
where
    R: SymbolicationResult,
    H: FileAndPathHelper<'h>,
{
    let get_file = |path| helper.open_file(&FileLocation::Path(path));

    let root_contents = get_file(dyld_cache_path.into()).await.map_err(|e| {
        Error::HelperErrorDuringOpenFile(dyld_cache_path.to_string_lossy().to_string(), e)
    })?;
    let root_contents = FileContentsWrapper::new(root_contents);

    let base_path = FileLocation::Path(dyld_cache_path.to_owned()).to_base_path();
    let dyld_cache_path = dyld_cache_path.to_string_lossy();

    let mut subcache_contents = Vec::new();
    for subcache_index in 1.. {
        // Find the subcache at dyld_shared_cache_arm64e.1 or dyld_shared_cache_arm64e.01
        let subcache_path = format!("{}.{}", dyld_cache_path, subcache_index);
        let subcache_path2 = format!("{}.{:02}", dyld_cache_path, subcache_index);
        let subcache = match get_file(subcache_path.into()).await {
            Ok(subcache) => subcache,
            Err(_) => match get_file(subcache_path2.into()).await {
                Ok(subcache) => subcache,
                Err(_) => break,
            },
        };
        subcache_contents.push(FileContentsWrapper::new(subcache));
    }
    let symbols_subcache_path = format!("{}.symbols", dyld_cache_path);
    if let Ok(subcache) = get_file(symbols_subcache_path.into()).await {
        subcache_contents.push(FileContentsWrapper::new(subcache));
    };

    let subcache_contents_refs: Vec<&FileContentsWrapper<H::F>> =
        subcache_contents.iter().collect();
    let cache = object::read::macho::DyldCache::<Endianness, _>::parse(
        &root_contents,
        &subcache_contents_refs,
    )
    .map_err(Error::DyldCacheParseError)?;
    let image = match cache.images().find(|image| image.path() == Ok(dylib_path)) {
        Some(image) => image,
        None => return Err(Error::NoMatchingDyldCacheImagePath(dylib_path.to_string())),
    };

    let object = image.parse_object().map_err(Error::MachOHeaderParseError)?;

    let (data, header_offset) = image
        .image_data_and_offset()
        .map_err(Error::MachOHeaderParseError)?;
    let macho_data = MachOData::new(data, header_offset, object.is_64());
    let symbol_map =
        get_symbol_map_from_macho_object(&object, data, macho_data, &base_path, query.clone())?;

    let addresses = match query.result_kind {
        SymbolicationResultKind::AllSymbols => return Ok(R::from_full_map(symbol_map.to_map())),
        SymbolicationResultKind::SymbolsForAddresses(addresses) => addresses,
    };

    let mut symbolication_result = R::for_addresses(addresses);
    symbolication_result.set_total_symbol_count(symbol_map.symbol_count() as u32);
    let uplooker = symbol_map.make_uplooker();

    for &address in addresses {
        if let Some(address_info) = uplooker.lookup(address) {
            symbolication_result.add_address_symbol(
                address,
                address_info.symbol.address,
                address_info.symbol.name,
                address_info.symbol.size,
            );
        }
    }

    Ok(symbolication_result)
}

pub fn get_symbol_map_from_macho_object<'a, 'data: 'file, 'file, RR: ReadRef<'data>>(
    macho_file: &'file File<'data, RR>,
    file_data: RR,
    macho_data: MachOData<'data, RR>,
    base_path: &BasePath,
    query: SymbolicationQuery<'a>,
) -> Result<SymbolMap<'data, <File<'data, RR> as Object<'data, 'file>>::Symbol, RR>, Error> {
    let file_debug_id = match debug_id_for_object(macho_file) {
        Some(debug_id) => debug_id,
        None => return Err(Error::InvalidInputError("Missing mach-o uuid")),
    };
    if file_debug_id != query.debug_id {
        return Err(Error::UnmatchedDebugId(file_debug_id, query.debug_id));
    }

    // Get function start addresses from LC_FUNCTION_STARTS
    let mut function_starts = macho_data.get_function_starts()?;

    // and from __unwind_info.
    if let Some(unwind_info) = macho_file
        .section_by_name_bytes(b"__unwind_info")
        .and_then(|s| s.data().ok())
        .and_then(|d| UnwindInfo::parse(d).ok())
    {
        let function_starts = function_starts.get_or_insert_with(Vec::new);
        let mut iter = unwind_info.functions();
        while let Ok(Some(function)) = iter.next() {
            function_starts.push(function.start_address);
        }
    }

    let path_mapper = PathMapper::new(base_path);
    Ok(SymbolMap::new(
        macho_file,
        file_data,
        path_mapper,
        function_starts.as_deref(),
        None,
    ))
}

pub async fn get_symbolication_result<'a, 'b, 'h, F, H, R>(
    base_path: &BasePath,
    file_contents: FileContentsWrapper<F>,
    file_range: Option<(u64, u64)>,
    query: SymbolicationQuery<'a>,
    helper: &'h H,
) -> Result<R, Error>
where
    F: FileContents,
    R: SymbolicationResult,
    H: FileAndPathHelper<'h, F = F>,
{
    let file_contents_ref = &file_contents;
    let range = match file_range {
        Some((start, size)) => file_contents_ref.range(start, size),
        None => file_contents_ref.full_range(),
    };

    let macho_file = File::parse(range).map_err(Error::MachOHeaderParseError)?;

    let macho_data = MachOData::new(range, 0, macho_file.is_64());
    let symbol_map =
        get_symbol_map_from_macho_object(&macho_file, range, macho_data, base_path, query.clone())?;

    let addresses = match query.result_kind {
        SymbolicationResultKind::AllSymbols => return Ok(R::from_full_map(symbol_map.to_map())),
        SymbolicationResultKind::SymbolsForAddresses(addresses) => addresses,
    };

    let mut symbolication_result = R::for_addresses(addresses);
    symbolication_result.set_total_symbol_count(symbol_map.symbol_count() as u32);
    let uplooker = symbol_map.make_uplooker();

    // We need to gather debug info for the supplied addresses.
    // On macOS, debug info can either be in this macho_file, or it can be in
    // other files ("external objects").
    // The following code is written in a way that handles a mixture of the two;
    // it gathers what debug info it can find in the current object, and also
    // makes a list of external object references. Then it reads those external objects
    // and keeps gathering more data until it has it all.
    // In theory, external objects can reference other external objects. The code
    // below handles such nesting, but it's unclear if this can happen in practice.

    // The original addresses which our caller wants to look up are relative to
    // the "root object". In the external objects they'll be at a different address.

    let image_base = relative_address_base(&macho_file);
    let mut path_mapper = PathMapper::new(base_path);

    let object_map = macho_file.object_map();

    let mut current_object: Option<ExternalFile<F>> = None;

    let mut external_addresses = Vec::new();

    for &address in addresses {
        if let Some(address_info) = uplooker.lookup(address) {
            symbolication_result.add_address_symbol(
                address,
                address_info.symbol.address,
                address_info.symbol.name,
                address_info.symbol.size,
            );
            if let Some(frames) = address_info.frames {
                symbolication_result.add_address_debug_info(address, AddressDebugInfo { frames });
            } else {
                external_addresses.push(address);
            }
        }
    }

    for address in external_addresses {
        let vmaddr = image_base + u64::from(address);
        if let Some(entry) = object_map.get(vmaddr) {
            let external_object_name = entry.object(&object_map);
            let external_object_name = std::str::from_utf8(external_object_name).unwrap();
            let offset_from_function_start = (vmaddr - entry.address()) as u32;
            let (file_name, name_in_archive) = match external_object_name.find('(') {
                Some(index) => {
                    // This is an "archive" reference of the form
                    // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../../js/src/build/libjs_static.a(Unified_cpp_js_src13.o)"
                    let (path, paren_rest) = external_object_name.split_at(index);
                    let name_in_archive = paren_rest.trim_start_matches('(').trim_end_matches(')');
                    (path, Some(name_in_archive))
                }
                None => {
                    // This is a reference to a regular object file. Example:
                    // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../components/sessionstore/Unified_cpp_sessionstore0.o"
                    (external_object_name, None)
                }
            };
            let external_object = match &current_object {
                Some(external_object) if external_object.name == file_name => external_object,
                _ => {
                    let file_contents = match helper
                        .open_file(&FileLocation::Path(file_name.into()))
                        .await
                    {
                        Ok(file) => FileContentsWrapper::new(file),
                        Err(_) => continue,
                    };

                    let archive_members_by_name: HashMap<Vec<u8>, (u64, u64)> =
                        match ArchiveFile::parse(&file_contents) {
                            Ok(archive) => archive
                                .members()
                                .filter_map(|member| match member {
                                    Ok(member) => {
                                        Some((member.name().to_owned(), member.file_range()))
                                    }
                                    Err(_) => None,
                                })
                                .collect(),
                            Err(_) => HashMap::new(),
                        };
                    current_object = Some(ExternalFile {
                        name: file_name,
                        file_contents,
                        archive_members_by_name,
                    });
                    current_object.as_ref().unwrap()
                }
            };
            if let Ok(stuff) = external_object.make_stuff(name_in_archive) {
                let fun_name = entry.name();
                if let Some(fun_address) = stuff.symbol_addresses.get(fun_name) {
                    let address_in_external_object =
                        fun_address + offset_from_function_start as u64;
                    let uplooker = stuff.make_uplooker();
                    let frames = get_frames(
                        address_in_external_object,
                        uplooker.context.as_ref(),
                        &mut path_mapper,
                    );
                    if let Some(frames) = frames {
                        symbolication_result
                            .add_address_debug_info(address, AddressDebugInfo { frames });
                    }
                }
            }
        }
    }

    Ok(symbolication_result)
}

// Disabled due to "higher-ranked lifetime error"
#[cfg(any())]
#[test]
fn test_future_send() {
    fn assert_is_send<T: Send>(_f: T) {}
    fn wrapper<'a, 'b, F, H, R>(
        base_path: &BasePath,
        file_contents: FileContentsWrapper<F>,
        file_range: Option<(u64, u64)>,
        query: SymbolicationQuery<'a>,
        helper: &'static H,
    ) where
        F: FileContents + Send + Sync,
        H: FileAndPathHelper<'static, F = F>,
        R: SymbolicationResult + Send,
        <H as FileAndPathHelper<'static>>::OpenFileFuture: Send,
        H: Sync,
    {
        let f = get_symbolication_result::<F, H, R>(
            base_path,
            file_contents,
            file_range,
            query,
            helper,
        );
        assert_is_send(f);
    }
}

struct ExternalFileStuff<'extobjdata, R: ReadRef<'extobjdata>> {
    section_data: SectionDataNoCopy<'extobjdata, R>,
    symbol_addresses: HashMap<&'extobjdata [u8], u64>,
}

impl<'extobjdata, R: ReadRef<'extobjdata>> ExternalFileStuff<'extobjdata, R> {
    pub fn make_uplooker(&self) -> ExternalObjectUplooker<'_> {
        let context = self.section_data.make_addr2line_context().ok();
        ExternalObjectUplooker { context }
    }
}

struct ExternalObjectUplooker<'a> {
    context: Option<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
}

struct ExternalFile<'data, F: FileContents> {
    name: &'data str,
    file_contents: FileContentsWrapper<F>,
    archive_members_by_name: HashMap<Vec<u8>, (u64, u64)>,
}

impl<'data, F: FileContents> ExternalFile<'data, F> {
    pub fn make_stuff<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalFileStuff<'s, RangeReadRef<&'s FileContentsWrapper<F>>>, Error> {
        let data = &self.file_contents;
        let data = match name_in_archive {
            Some(name_in_archive) => {
                let (start, size) = self
                    .archive_members_by_name
                    .get(name_in_archive.as_bytes())
                    .ok_or_else(|| Error::FileNotInArchive(name_in_archive.to_owned()))?;
                RangeReadRef::new(data, *start, *size)
            }
            None => RangeReadRef::new(data, 0, data.len()),
        };
        let object_file = File::parse(data).map_err(Error::MachOHeaderParseError)?;
        let section_data = SectionDataNoCopy::from_object(
            RangeReadRef::new(data, 0, data.len().unwrap()),
            &object_file,
        );
        let symbol_addresses = object_file
            .symbols()
            .filter_map(|symbol| {
                let name = symbol.name_bytes().ok()?;
                let address = symbol.address();
                Some((name, address))
            })
            .collect();
        Ok(ExternalFileStuff {
            section_data,
            symbol_addresses,
        })
    }
}
pub struct MachOData<'data, R: ReadRef<'data>> {
    data: R,
    header_offset: u64,
    is_64: bool,
    _phantom: PhantomData<&'data ()>,
}

impl<'data, R: ReadRef<'data>> MachOData<'data, R> {
    pub fn new(data: R, header_offset: u64, is_64: bool) -> Self {
        Self {
            data,
            header_offset,
            is_64,
            _phantom: PhantomData,
        }
    }

    /// Read the list of function start addresses from the LC_FUNCTION_STARTS mach-O load command.
    /// This information is usually present even in stripped binaries. It's a uleb128 encoded list
    /// of deltas between the function addresses, with a zero delta terminator.
    /// We use this information to improve symbolication for stripped binaries: It allows us to
    /// group addresses from the same function into the same (synthesized) "symbol". It also allows
    /// better results for binaries with partial symbol tables, because it tells us where the
    /// functions with symbols end. This means that those symbols don't "overreach" to cover
    /// addresses after their function - instead, they get correctly terminated by a symbol-less
    /// function's start address.
    pub fn get_function_starts(&self) -> Result<Option<Vec<u32>>, Error> {
        let data = self
            .function_start_data()
            .map_err(Error::MachOHeaderParseError)?;
        let data = if let Some(data) = data {
            data
        } else {
            return Ok(None);
        };
        let mut function_starts = Vec::new();
        let mut prev_address = 0;
        let mut bytes = data;
        while let Some((delta, rest)) = read_uleb128(bytes) {
            if delta == 0 {
                break;
            }
            bytes = rest;
            let address = prev_address + delta;
            function_starts.push(address as u32);
            prev_address = address;
        }

        Ok(Some(function_starts))
    }

    fn load_command_iter<M: MachHeader>(
        &self,
    ) -> object::read::Result<(M::Endian, LoadCommandIterator<M::Endian>)> {
        let header = M::parse(self.data, self.header_offset)?;
        let endian = header.endian()?;
        let load_commands = header.load_commands(endian, self.data, self.header_offset)?;
        Ok((endian, load_commands))
    }

    fn function_start_data(&self) -> object::read::Result<Option<&'data [u8]>> {
        let (endian, mut commands) = if self.is_64 {
            self.load_command_iter::<MachHeader64<Endianness>>()?
        } else {
            self.load_command_iter::<MachHeader32<Endianness>>()?
        };
        while let Ok(Some(command)) = commands.next() {
            if command.cmd() == macho::LC_FUNCTION_STARTS {
                let command: &LinkeditDataCommand<_> = command.data()?;
                let dataoff: u64 = command.dataoff.get(endian).into();
                let datasize: u64 = command.datasize.get(endian).into();
                let data = self.data.read_bytes_at(dataoff, datasize).ok();
                return Ok(data);
            }
        }
        Ok(None)
    }
}

fn read_uleb128(mut bytes: &[u8]) -> Option<(u64, &[u8])> {
    const CONTINUATION_BIT: u8 = 1 << 7;

    let mut result = 0;
    let mut shift = 0;

    while !bytes.is_empty() {
        let byte = bytes[0];
        bytes = &bytes[1..];
        if shift == 63 && byte != 0x00 && byte != 0x01 {
            return None;
        }

        let low_bits = u64::from(byte & !CONTINUATION_BIT);
        result |= low_bits << shift;

        if byte & CONTINUATION_BIT == 0 {
            return Some((result, bytes));
        }

        shift += 7;
    }
    None
}
