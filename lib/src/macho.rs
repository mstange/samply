use crate::dwarf::{
    collect_dwarf_address_debug_data, make_address_pairs_for_root_object, AddressPair,
};
use crate::error::{GetSymbolsError, Result};
use crate::object_debugid::debug_id_for_object;
use crate::path_mapper::PathMapper;
use crate::shared::{
    get_symbolication_result_for_addresses_from_object, object_to_map, FileAndPathHelper,
    FileContents, FileContentsWrapper, FileLocation, RangeReadRef, SymbolicationQuery,
    SymbolicationResult, SymbolicationResultKind,
};
use debugid::DebugId;
use macho_unwind_info::UnwindInfo;
use object::macho::{self, LinkeditDataCommand, MachHeader32, MachHeader64};
use object::read::macho::{FatArch, LoadCommandIterator, MachHeader};
use object::read::{archive::ArchiveFile, File, Object, ObjectSection, ObjectSymbol};
use object::{Endianness, ObjectMapEntry, ReadRef};
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Returns the (offset, size) in the fat binary file for the object that matches
// breakpad_id, if found.
pub fn get_arch_range(
    file_contents: &FileContentsWrapper<impl FileContents>,
    arches: &[impl FatArch],
    debug_id: DebugId,
) -> Result<(u64, u64)> {
    let mut debug_ids = Vec::new();
    let mut errors = Vec::new();

    for fat_arch in arches {
        let range = fat_arch.file_range();
        let (start, size) = range;
        let file = File::parse(file_contents.range(start, size))
            .map_err(GetSymbolsError::MachOHeaderParseError)?;
        match debug_id_for_object(&file) {
            Some(di) => {
                if di == debug_id {
                    return Ok(range);
                }
                debug_ids.push(di);
            }
            None => {
                errors.push(GetSymbolsError::InvalidInputError("Missing mach-O UUID"));
            }
        }
    }
    Err(GetSymbolsError::NoMatchMultiArch(debug_ids, errors))
}

pub async fn try_get_symbolication_result_from_dyld_shared_cache<'h, R, H>(
    query: SymbolicationQuery<'_>,
    dyld_cache_path: &Path,
    dylib_path: &str,
    helper: &'h H,
) -> Result<R>
where
    R: SymbolicationResult,
    H: FileAndPathHelper<'h>,
{
    let root_contents = helper
        .open_file(&FileLocation::Path(dyld_cache_path.to_path_buf()))
        .await
        .map_err(|e| {
            GetSymbolsError::HelperErrorDuringOpenFile(
                dyld_cache_path.to_string_lossy().to_string(),
                e,
            )
        })?;
    let root_contents = FileContentsWrapper::new(root_contents);

    let dyld_cache_path = dyld_cache_path.to_string_lossy();

    let mut subcache_contents = Vec::new();
    for subcache_index in 1.. {
        let subcache_path = format!("{}.{}", dyld_cache_path, subcache_index);
        match helper
            .open_file(&FileLocation::Path(subcache_path.into()))
            .await
        {
            Ok(subcache) => subcache_contents.push(FileContentsWrapper::new(subcache)),
            Err(_) => break,
        };
    }
    let symbols_subcache_path = format!("{}.symbols", dyld_cache_path);
    if let Ok(subcache) = helper
        .open_file(&FileLocation::Path(symbols_subcache_path.into()))
        .await
    {
        subcache_contents.push(FileContentsWrapper::new(subcache));
    };

    let subcache_contents_refs: Vec<&FileContentsWrapper<H::F>> =
        subcache_contents.iter().collect();
    let cache = object::read::macho::DyldCache::<Endianness, _>::parse(
        &root_contents,
        &subcache_contents_refs,
    )
    .map_err(GetSymbolsError::DyldCacheParseError)?;
    let image = match cache.images().find(|image| image.path() == Ok(dylib_path)) {
        Some(image) => image,
        None => {
            return Err(GetSymbolsError::NoMatchingDyldCacheImagePath(
                dylib_path.to_string(),
            ))
        }
    };

    let object = image
        .parse_object()
        .map_err(GetSymbolsError::MachOHeaderParseError)?;

    let (data, header_offset) = image
        .image_data_and_offset()
        .map_err(GetSymbolsError::MachOHeaderParseError)?;
    let macho_data = MachOData::new(data, header_offset, object.is_64());
    get_symbolication_result_from_macho_object(&object, macho_data, query)
}

pub fn get_symbolication_result_from_macho_object<'a, 'data, R, RR: ReadRef<'data>>(
    macho_file: &File<'data, RR>,
    macho_data: MachOData<'data, RR>,
    query: SymbolicationQuery<'a>,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let file_debug_id = match debug_id_for_object(macho_file) {
        Some(debug_id) => debug_id,
        None => return Err(GetSymbolsError::InvalidInputError("Missing mach-o uuid")),
    };
    if file_debug_id != query.debug_id {
        return Err(GetSymbolsError::UnmatchedDebugId(
            file_debug_id,
            query.debug_id,
        ));
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

    match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            let map = object_to_map(macho_file, function_starts.as_deref());
            Ok(R::from_full_map(map))
        }
        SymbolicationResultKind::SymbolsForAddresses { addresses, .. } => {
            Ok(get_symbolication_result_for_addresses_from_object(
                addresses,
                macho_file,
                function_starts.as_deref(),
                None,
            ))
        }
    }
}

pub async fn get_symbolication_result<'a, 'b, 'h, R>(
    file_contents: FileContentsWrapper<impl FileContents>,
    file_range: Option<(u64, u64)>,
    query: SymbolicationQuery<'a>,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let file_contents_ref = &file_contents;
    let range = match file_range {
        Some((start, size)) => file_contents_ref.range(start, size),
        None => file_contents_ref.full_range(),
    };

    let macho_file = File::parse(range).map_err(GetSymbolsError::MachOHeaderParseError)?;

    let macho_data = MachOData::new(range, 0, macho_file.is_64());
    let mut symbolication_result =
        get_symbolication_result_from_macho_object(&macho_file, macho_data, query.clone())?;

    let addresses = match query.result_kind {
        SymbolicationResultKind::SymbolsForAddresses {
            with_debug_info: true,
            addresses,
        } => addresses,
        _ => return Ok(symbolication_result),
    };

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
    // To correctly associate the found information with the original address,
    // we need to track an AddressPair, which has both the original address and
    // the address that we need to look up in the current object.

    let addresses_in_root_object = make_address_pairs_for_root_object(addresses, &macho_file);
    let mut path_mapper = PathMapper::new();
    let mut object_references = VecDeque::new();
    collect_debug_info_and_object_references(
        range,
        &macho_file,
        &addresses_in_root_object,
        &mut symbolication_result,
        &mut object_references,
        &mut path_mapper,
    );

    // We are now done with the "root object" and can discard its data.
    drop(macho_file);
    drop(file_contents);

    // Collect the debug info from the external references.
    traverse_object_references_and_collect_debug_info(
        object_references,
        &mut symbolication_result,
        helper,
        &mut path_mapper,
    )
    .await?;

    Ok(symbolication_result)
}

async fn traverse_object_references_and_collect_debug_info<'h>(
    object_references: VecDeque<ObjectReference>,
    symbolication_result: &mut impl SymbolicationResult,
    helper: &'h impl FileAndPathHelper<'h>,
    path_mapper: &mut PathMapper<()>,
) -> Result<()> {
    // Do a breadth-first-traversal of the external debug info reference tree.
    // We do this using a while loop and a VecDeque rather than recursion, because
    // async functions can't easily recurse.
    let mut remaining_object_references = object_references;
    while let Some(obj_ref) = remaining_object_references.pop_front() {
        let path = obj_ref.path().to_owned();
        let file_contents = match helper.open_file(&FileLocation::Path(path)).await {
            Ok(data) => FileContentsWrapper::new(data),
            Err(_) => {
                // We probably couldn't find the file, but that's fine.
                // It would be good to collect this error somewhere.
                continue;
            }
        };

        for (data, functions) in obj_ref.into_objects(&file_contents)?.into_iter() {
            let macho_file = File::parse(data).map_err(GetSymbolsError::MachOHeaderParseError)?;
            let addresses_in_this_object = translate_addresses_to_object(&macho_file, functions);
            collect_debug_info_and_object_references(
                data,
                &macho_file,
                &addresses_in_this_object,
                symbolication_result,
                &mut remaining_object_references,
                path_mapper,
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionsWithAddresses {
    /// Keys are byte strings of the function name.
    /// Values are the addresses under that function.
    map: HashMap<Vec<u8>, Vec<AddressWithOffset>>,
}

impl FunctionsWithAddresses {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, symbol_name: &[u8], addresses: Vec<AddressWithOffset>) {
        self.map.insert(symbol_name.to_owned(), addresses);
    }

    pub fn get_and_remove_addresses_for_function(
        &mut self,
        symbol_name: &str,
    ) -> Option<Vec<AddressWithOffset>> {
        self.map.remove(symbol_name.as_bytes())
    }
}

fn translate_addresses_to_object<'data: 'file, 'file, O>(
    macho_file: &'file O,
    mut functions: FunctionsWithAddresses,
) -> Vec<AddressPair>
where
    O: Object<'data, 'file>,
{
    let mut addresses_in_this_object = Vec::new();
    for symbol in macho_file.symbols() {
        if let Ok(symbol_name) = symbol.name() {
            if let Some(addresses) = functions.get_and_remove_addresses_for_function(symbol_name) {
                for AddressWithOffset {
                    original_relative_address,
                    offset_from_function_start,
                } in addresses
                {
                    let vmaddr_in_this_object =
                        symbol.address() + offset_from_function_start as u64;
                    addresses_in_this_object.push(AddressPair {
                        original_relative_address,
                        vmaddr_in_this_object,
                    });
                }
            }
        }
    }
    addresses_in_this_object.sort_by_key(|ap| ap.vmaddr_in_this_object);
    addresses_in_this_object
}

enum ObjectReference {
    Regular {
        path: PathBuf,
        functions: FunctionsWithAddresses,
    },
    Archive {
        path: PathBuf,
        archive_info: HashMap<String, FunctionsWithAddresses>,
    },
}

impl ObjectReference {
    fn path(&self) -> &Path {
        match self {
            ObjectReference::Regular { path, .. } => path,
            ObjectReference::Archive { path, .. } => path,
        }
    }

    fn into_objects<'a, 'b, R: ReadRef<'b>>(
        self,
        data: R,
    ) -> Result<Vec<(RangeReadRef<'b, R>, FunctionsWithAddresses)>> {
        match self {
            ObjectReference::Regular { functions, .. } => Ok(vec![(
                RangeReadRef::new(data, 0, data.len().unwrap_or(0)),
                functions,
            )]),
            ObjectReference::Archive {
                path, archive_info, ..
            } => {
                let archive = ArchiveFile::parse(data)
                    .map_err(|x| GetSymbolsError::ArchiveParseError(path.clone(), Box::new(x)))?;
                let archive_members_by_name: HashMap<Vec<u8>, (u64, u64)> = archive
                    .members()
                    .filter_map(|member| match member {
                        Ok(member) => Some((member.name().to_owned(), member.file_range())),
                        Err(_) => None,
                    })
                    .collect();
                let v: Vec<_> = archive_info
                    .into_iter()
                    .filter_map(|(name_in_archive, functions)| {
                        archive_members_by_name
                            .get(name_in_archive.as_bytes())
                            .map(|&(start, size)| (RangeReadRef::new(data, start, size), functions))
                    })
                    .collect();
                Ok(v)
            }
        }
    }
}

/// addresses must be sorted by vmaddr_in_this_object
fn collect_debug_info_and_object_references<'data: 'file, 'file, 'a, O, R>(
    file_data: RangeReadRef<'data, impl ReadRef<'data>>,
    macho_file: &'file O,
    addresses: &[AddressPair],
    symbolication_result: &mut R,
    remaining_object_references: &mut VecDeque<ObjectReference>,
    path_mapper: &mut PathMapper<()>,
) where
    O: Object<'data, 'file>,
    R: SymbolicationResult,
{
    let object_map = macho_file.object_map();
    let objects = object_map.objects();
    let mut object_map_symbols: Vec<_> = object_map.symbols().to_owned();
    object_map_symbols.sort_by_key(|symbol| symbol.address());
    let (external_funs_by_object, internal_addresses) =
        match_funs_to_addresses(&object_map_symbols, addresses);
    collect_dwarf_address_debug_data(
        file_data,
        macho_file,
        &internal_addresses,
        symbolication_result,
        path_mapper,
    );

    let mut archives = HashMap::new();

    for (object_index, functions) in external_funs_by_object.into_iter() {
        let object_name = std::str::from_utf8(objects[object_index]).unwrap();
        match object_name.find('(') {
            Some(index) => {
                // This is an "archive" reference of the form
                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../../js/src/build/libjs_static.a(Unified_cpp_js_src13.o)"
                let (path, paren_rest) = object_name.split_at(index);
                let path: PathBuf = path.into();
                let name_in_archive = paren_rest
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .to_string();
                let archive_info = archives.entry(path).or_insert_with(HashMap::new);
                archive_info.insert(name_in_archive, functions);
            }
            None => {
                // This is a reference to a regular object file. Example:
                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../components/sessionstore/Unified_cpp_sessionstore0.o"
                let path: PathBuf = object_name.into();
                remaining_object_references.push_back(ObjectReference::Regular { path, functions });
            }
        }
    }

    for (path, archive_info) in archives.into_iter() {
        remaining_object_references.push_back(ObjectReference::Archive { path, archive_info });
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct AddressWithOffset {
    original_relative_address: u32,
    offset_from_function_start: u32,
}

/// Assign each address to a function in an object.
/// function_symbols must be sorted by function.address().
/// addresses must be sorted by vmaddr_in_this_object.
/// This function is implemented as a linear single pass over both slices.
/// Also returns a sorted Vec of AddressPairs for addresses that were not found
/// in an external object.
fn match_funs_to_addresses<'a>(
    function_symbols: &[ObjectMapEntry<'a>],
    addresses: &[AddressPair],
) -> (HashMap<usize, FunctionsWithAddresses>, Vec<AddressPair>) {
    let mut external_funs_by_object: HashMap<usize, FunctionsWithAddresses> = HashMap::new();
    let mut internal_addresses = Vec::new();
    let mut addr_iter = addresses.iter();
    let mut cur_addr = addr_iter.next();
    let mut fun_iter = function_symbols.iter();
    let mut cur_fun = fun_iter.next();
    let mut cur_fun_addresses = Vec::new();

    let mut flush_cur_fun = |object_index, fun_name, addresses: Vec<AddressWithOffset>| {
        if !addresses.is_empty() {
            external_funs_by_object
                .entry(object_index)
                .or_insert_with(FunctionsWithAddresses::new)
                .insert(fun_name, addresses);
        }
    };

    while let (Some(address_pair), Some(fun)) = (cur_addr, cur_fun) {
        let original_relative_address = address_pair.original_relative_address;
        let vmaddr_in_this_object = address_pair.vmaddr_in_this_object;
        if fun.address() > vmaddr_in_this_object {
            internal_addresses.push(address_pair.clone());
            // Advance cur_addr.
            cur_addr = addr_iter.next();
            continue;
        }
        if vmaddr_in_this_object >= fun.address() + fun.size() {
            // Advance cur_fun.
            flush_cur_fun(fun.object_index(), fun.name(), cur_fun_addresses);
            cur_fun = fun_iter.next();
            cur_fun_addresses = Vec::new();
            continue;
        }
        // Now the following is true:
        // fun.address() <= vmaddr_in_this_object && vmaddr_in_this_object < fun.address() + fun.size()
        let offset_from_function_start = (vmaddr_in_this_object - fun.address()) as u32;
        cur_fun_addresses.push(AddressWithOffset {
            original_relative_address,
            offset_from_function_start,
        });
        // Advance cur_addr.
        cur_addr = addr_iter.next();
    }

    // Flush addresses for the final function.
    if let Some(fun) = cur_fun {
        flush_cur_fun(fun.object_index(), fun.name(), cur_fun_addresses);
    }

    // Consume remaining addresses.
    while let Some(address_pair) = cur_addr {
        internal_addresses.push(address_pair.clone());
        cur_addr = addr_iter.next();
    }

    (external_funs_by_object, internal_addresses)
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
    pub fn get_function_starts(&self) -> Result<Option<Vec<u32>>> {
        let data = self
            .function_start_data()
            .map_err(GetSymbolsError::MachOHeaderParseError)?;
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
