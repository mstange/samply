use crate::dwarf::{collect_dwarf_address_debug_data, AddressPair};
use crate::error::{GetSymbolsError, Result};
use crate::shared::{
    object_to_map, FileAndPathHelper, FileContents, FileContentsWrapper, SymbolicationQuery,
    SymbolicationResult,
};
use object::read::{archive::ArchiveFile, File, FileKind, Object, ObjectSymbol};
use object::{
    macho::{MachHeader32, MachHeader64},
    ReadRef,
};
use object::{
    read::macho::{FatArch, MachHeader},
    Endianness,
};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Returns the (offset, size) in the fat binary file for the object that matches
// breakpad_id, if found.
pub fn get_arch_range(
    file_contents: &FileContentsWrapper<impl FileContents>,
    arches: &[impl FatArch],
    breakpad_id: &str,
) -> Result<(u64, u64)> {
    let mut uuids = Vec::new();
    let mut errors = Vec::new();

    for fat_arch in arches {
        let range = fat_arch.file_range();
        let (start, size) = range;
        match get_macho_uuid(file_contents.range(start, size)) {
            Ok(uuid) => {
                if uuid == breakpad_id {
                    return Ok(range);
                }
                uuids.push(uuid);
            }
            Err(err) => {
                errors.push(err);
            }
        }
    }
    Err(GetSymbolsError::NoMatchMultiArch(uuids, errors))
}

pub fn get_macho_uuid<'a, R: ReadRef<'a>>(data: R) -> Result<String> {
    let helper = || {
        let file_kind = FileKind::parse(data)?;
        match file_kind {
            FileKind::MachO32 => {
                let header = MachHeader32::<Endianness>::parse(data)?;
                header.uuid(header.endian()?, data)
            }
            FileKind::MachO64 => {
                let header = MachHeader64::<Endianness>::parse(data)?;
                header.uuid(header.endian()?, data)
            }
            _ => panic!("Unexpected file kind {:?}", file_kind),
        }
    };
    match helper() {
        Ok(Some(uuid)) => Ok(format!("{:X}0", Uuid::from_bytes(uuid).to_simple())),
        Ok(None) => Err(GetSymbolsError::InvalidInputError("Missing mach-o uuid")),
        Err(err) => Err(GetSymbolsError::MachOHeaderParseError(err)),
    }
}

pub async fn get_symbolication_result<'a, 'b, R>(
    file_contents: FileContentsWrapper<impl FileContents>,
    file_range: Option<(u64, u64)>,
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let file_contents_ref = &file_contents;
    let range = match file_range {
        Some((start, size)) => file_contents_ref.range(start, size),
        None => file_contents_ref.full_range(),
    };

    let uuid = get_macho_uuid(range)?;
    if uuid != query.breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            uuid,
            query.breakpad_id.to_string(),
        ));
    }

    let macho_file = File::parse(range).map_err(|e| GetSymbolsError::MachOHeaderParseError(e))?;
    let map = object_to_map(&macho_file);
    let addresses = query.addresses;
    let mut symbolication_result = R::from_full_map(map, addresses);

    if !R::result_kind().wants_debug_info_for_addresses() {
        return Ok(symbolication_result);
    }

    let addresses_in_root_object = make_address_pairs_for_root_object(addresses, &macho_file);
    let mut object_references = VecDeque::new();
    collect_debug_info_and_object_references(
        &macho_file,
        &addresses_in_root_object,
        &mut symbolication_result,
        &mut object_references,
    )?;

    // We are now done with the "root object" and can discard its data.
    drop(macho_file);
    drop(file_contents);

    // Collect the debug info from the external references.
    traverse_object_references_and_collect_debug_info(
        object_references,
        &mut symbolication_result,
        helper,
    )
    .await?;

    Ok(symbolication_result)
}

async fn traverse_object_references_and_collect_debug_info(
    object_references: VecDeque<ObjectReference>,
    symbolication_result: &mut impl SymbolicationResult,
    helper: &impl FileAndPathHelper,
) -> Result<()> {
    // Do a breadth-first-traversal of the external debug info reference tree.
    // We do this using a while loop and a VecDeque rather than recursion, because
    // async functions can't easily recurse.
    let mut remaining_object_references = object_references;
    while let Some(obj_ref) = remaining_object_references.pop_front() {
        let path = obj_ref.path();
        let file_contents = match helper.open_file(path).await {
            Ok(data) => FileContentsWrapper::new(data),
            Err(_) => {
                // We probably couldn't find the file, but that's fine.
                // It would be good to collect this error somewhere.
                continue;
            }
        };

        match obj_ref {
            ObjectReference::Regular { functions, .. } => {
                let macho_file = File::parse(&file_contents)
                    .or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;
                let addresses_in_this_object =
                    translate_addresses_to_object(&macho_file, functions);
                collect_debug_info_and_object_references(
                    &macho_file,
                    &addresses_in_this_object,
                    symbolication_result,
                    &mut remaining_object_references,
                )?;
            }
            ObjectReference::Archive {
                path, archive_info, ..
            } => {
                let archive = ArchiveFile::parse(&file_contents).or_else(|x| {
                    Err(GetSymbolsError::ArchiveParseError(
                        path.clone(),
                        Box::new(x),
                    ))
                })?;
                let archive_members_by_name: HashMap<Vec<u8>, &[u8]> = archive
                    .members()
                    .filter_map(|member| match member {
                        Ok(member) => member
                            .data(&file_contents)
                            .ok()
                            .map(|data| (member.name().to_owned(), data)),
                        Err(_) => None,
                    })
                    .collect();
                for (name_in_archive, functions) in archive_info {
                    let buffer = match archive_members_by_name.get(name_in_archive.as_bytes()) {
                        Some(buffer) => *buffer,
                        None => continue,
                    };
                    let macho_file = File::parse(buffer)
                        .or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;
                    let addresses_in_this_object =
                        translate_addresses_to_object(&macho_file, functions);
                    collect_debug_info_and_object_references(
                        &macho_file,
                        &addresses_in_this_object,
                        symbolication_result,
                        &mut remaining_object_references,
                    )?;
                }
            }
        };
    }

    Ok(())
}

fn make_address_pairs_for_root_object<'data: 'file, 'file, O>(
    addresses: &[u32],
    macho_file: &'file O,
) -> Vec<AddressPair>
where
    O: Object<'data, 'file>,
{
    use object::read::ObjectSegment;
    let vmaddr_of_text_segment = macho_file
        .segments()
        .find(|segment| segment.name() == Ok(Some("__TEXT")))
        .map(|segment| segment.address())
        .unwrap_or(0);

    // Look up addresses that don't have external debug info, and collect information
    // about the ones that do have external debug info.
    addresses
        .iter()
        .map(|a| AddressPair {
            original_address: *a,
            address_in_this_object: vmaddr_of_text_segment + *a as u64,
        })
        .collect()
}

fn translate_addresses_to_object<'data: 'file, 'file, O>(
    macho_file: &'file O,
    mut functions: HashMap<Vec<u8>, Vec<AddressWithOffset>>,
) -> Vec<AddressPair>
where
    O: Object<'data, 'file>,
{
    let mut addresses_in_this_object = Vec::new();
    for symbol in macho_file.symbols() {
        if let Ok(symbol_name) = symbol.name() {
            if let Some(addresses) = functions.remove(symbol_name.as_bytes()) {
                for AddressWithOffset {
                    original_address,
                    offset_from_function_start,
                } in addresses
                {
                    let address_in_this_object =
                        symbol.address() + offset_from_function_start as u64;
                    addresses_in_this_object.push(AddressPair {
                        original_address,
                        address_in_this_object,
                    });
                }
            }
        }
    }
    addresses_in_this_object.sort_by_key(|ap| ap.address_in_this_object);
    addresses_in_this_object
}

enum ObjectReference {
    Regular {
        path: PathBuf,
        functions: HashMap<Vec<u8>, Vec<AddressWithOffset>>,
    },
    Archive {
        path: PathBuf,
        archive_info: HashMap<String, HashMap<Vec<u8>, Vec<AddressWithOffset>>>,
    },
}

impl ObjectReference {
    fn path(&self) -> &Path {
        match self {
            ObjectReference::Regular { path, .. } => path,
            ObjectReference::Archive { path, .. } => path,
        }
    }
}

#[derive(Debug)]
struct ObjectMapSymbol<'a> {
    name: &'a [u8],
    address_range: std::ops::Range<u64>,
    object_index: Option<usize>,
}

fn collect_debug_info_and_object_references<'data: 'file, 'file, 'a, O, R>(
    macho_file: &'file O,
    addresses: &[AddressPair],
    symbolication_result: &mut R,
    remaining_object_references: &mut VecDeque<ObjectReference>,
) -> Result<()>
where
    O: Object<'data, 'file>,
    R: SymbolicationResult,
{
    let object_map = macho_file.object_map();
    let objects = object_map.objects();
    let mut object_map_symbols: Vec<_> = object_map
        .symbols()
        .iter()
        .map(|entry| {
            let address = entry.address();
            ObjectMapSymbol {
                name: entry.name(),
                address_range: address..(address + entry.size()),
                object_index: Some(entry.object_index()),
            }
        })
        .collect();
    object_map_symbols.sort_by_key(|f| f.address_range.start);
    let functions_with_addresses = match_funs_to_addresses(&object_map_symbols[..], addresses);
    let mut external_funs_by_object: HashMap<usize, HashMap<Vec<u8>, Vec<AddressWithOffset>>> =
        HashMap::new();
    let mut original_addresses_found_in_external_objects = BTreeSet::new();
    for MatchedFunctionWithAddresses {
        object_index,
        fun_name,
        addresses,
    } in functions_with_addresses.into_iter()
    {
        for AddressWithOffset {
            original_address, ..
        } in &addresses
        {
            original_addresses_found_in_external_objects.insert(*original_address);
        }
        external_funs_by_object
            .entry(object_index)
            .or_insert_with(HashMap::new)
            .insert(fun_name.to_owned(), addresses);
    }
    let internal_addresses: Vec<_> = addresses
        .iter()
        .cloned()
        .filter(|ap| !original_addresses_found_in_external_objects.contains(&ap.original_address))
        .collect();
    collect_dwarf_address_debug_data(macho_file, &internal_addresses, symbolication_result);

    let mut archives = HashMap::new();
    let mut regular_objects = HashMap::new();

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
                regular_objects.insert(path, functions);
            }
        }
    }

    for (path, archive_info) in archives.into_iter() {
        remaining_object_references.push_back(ObjectReference::Archive { path, archive_info });
    }
    for (path, functions) in regular_objects.into_iter() {
        remaining_object_references.push_back(ObjectReference::Regular { path, functions });
    }

    Ok(())
}

#[derive(Debug)]
struct AddressWithOffset {
    original_address: u32,
    offset_from_function_start: u32,
}

struct MatchedFunctionWithAddresses<'a> {
    object_index: usize,
    fun_name: &'a [u8],
    addresses: Vec<AddressWithOffset>,
}

// functions must be sorted by function.address_range.start
// addresses must be sorted
fn match_funs_to_addresses<'a, 'b, 'c>(
    functions: &'a [ObjectMapSymbol<'c>],
    addresses: &'b [AddressPair],
) -> Vec<MatchedFunctionWithAddresses<'c>> {
    let mut yo: Vec<MatchedFunctionWithAddresses> = Vec::new();
    let mut addr_iter = addresses.iter();
    let mut cur_addr = addr_iter.next();
    let mut fun_iter = functions.iter();
    let mut cur_fun = fun_iter.next();
    let mut cur_fun_is_last_vec_element = false;
    while let (Some(address_pair), Some(fun)) = (cur_addr, cur_fun) {
        let original_address = address_pair.original_address;
        let address_in_this_object = address_pair.address_in_this_object;
        if !(fun.address_range.start <= address_in_this_object) {
            // Advance address_pair.
            cur_addr = addr_iter.next();
            continue;
        }
        if !(address_in_this_object < fun.address_range.end || !fun.object_index.is_some()) {
            // Advance fun.
            cur_fun = fun_iter.next();
            cur_fun_is_last_vec_element = false;
            continue;
        }
        // Now the following is true:
        // fun.object_index.is_some() &&
        // fun.address_range.start <= address_in_this_object && address_in_this_object < fun.addr_range.end
        let offset_from_function_start = (address_in_this_object - fun.address_range.start) as u32;
        let address_with_offset = AddressWithOffset {
            original_address,
            offset_from_function_start,
        };
        if cur_fun_is_last_vec_element {
            yo.last_mut().unwrap().addresses.push(address_with_offset);
        } else {
            yo.push(MatchedFunctionWithAddresses {
                object_index: fun.object_index.unwrap(),
                fun_name: fun.name,
                addresses: vec![address_with_offset],
            });
            cur_fun_is_last_vec_element = true;
        }
        // Advance addr.
        cur_addr = addr_iter.next();
    }
    yo
}

#[derive(Debug)]
struct OriginObject<'a> {
    name: &'a str,
}

#[derive(Debug)]
struct Function<'a> {
    name: &'a str,
    address_range: std::ops::Range<u64>,
    object_index: Option<usize>,
}
