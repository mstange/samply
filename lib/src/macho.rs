use crate::dwarf::{collect_dwarf_address_debug_data, AddressPair};
use crate::error::{GetSymbolsError, Result};
use crate::shared::{
    object_to_map, FileAndPathHelper, OwnedFileData, SymbolicationQuery, SymbolicationResult,
    SymbolicationResultKind,
};
use addr2line::object;
use goblin::mach;
use object::read::{File, Object, ObjectSymbol};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub async fn get_symbolication_result_multiarch<'a, R>(
    owned_data: Arc<impl OwnedFileData>,
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let buffer = owned_data.get_data();
    let mut errors = vec![];
    let arches: Vec<_> = {
        let multi_arch = mach::MultiArch::new(buffer)?;
        multi_arch
            .iter_arches()
            .filter_map(std::result::Result::ok)
            .collect()
    };
    for fat_arch in arches {
        let slice = (fat_arch.offset as usize)..(fat_arch.offset as usize + fat_arch.size as usize);
        match get_symbolication_result(owned_data.clone(), Some(slice), query.clone(), helper).await
        {
            Ok(table) => return Ok(table),
            Err(err) => errors.push(err),
        }
    }
    Err(GetSymbolsError::NoMatchMultiArch(errors))
}

pub async fn get_symbolication_result<'a, 'b, R>(
    owned_data: Arc<impl OwnedFileData>,
    slice: Option<std::ops::Range<usize>>,
    query: SymbolicationQuery<'a>,
    helper: &impl FileAndPathHelper,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let mut buffer = owned_data.get_data();
    if let Some(slice) = slice {
        buffer = &buffer[slice];
    }
    let macho_file =
        File::parse(buffer).or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;

    let macho_id = match macho_file.mach_uuid() {
        Ok(Some(uuid)) => format!("{:X}0", Uuid::from_bytes(uuid).to_simple()),
        _ => {
            return Err(GetSymbolsError::InvalidInputError(
                "Could not get mach uuid",
            ))
        }
    };
    let SymbolicationQuery {
        breakpad_id,
        addresses,
        ..
    } = query;
    if macho_id != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            macho_id,
            breakpad_id.to_string(),
        ));
    }
    let map = object_to_map(&macho_file);
    let mut symbolication_result = R::from_full_map(map, addresses);

    if let SymbolicationResultKind::SymbolsForAddresses {
        with_debug_info: true,
    } = R::result_kind()
    {
        use object::read::ObjectSegment;
        let vmaddr_of_text_segment = macho_file
            .segments()
            .find(|segment| segment.name() == Ok(Some("__TEXT")))
            .map(|segment| segment.address())
            .unwrap_or(0);

        let mut remainder = VecDeque::new();

        // Look up addresses that don't have external debug info, and collect information
        // about the ones that do have external debug info.
        let addresses_in_this_object: Vec<_> = addresses
            .iter()
            .map(|a| AddressPair {
                original_address: *a,
                address_in_this_object: vmaddr_of_text_segment + *a as u64,
            })
            .collect();
        remainder.extend(collect_debug_info_and_remainder(
            &macho_file,
            &addresses_in_this_object,
            &mut symbolication_result,
        )?);
        // Now we're done with the original file. Release it.
        drop(owned_data);

        // Do a breadth-first-traversal of the external debug info reference tree.
        while let Some(obj_ref) = remainder.pop_front() {
            let path = obj_ref.path();
            let owned_data = match helper.read_file(path).await {
                Ok(data) => data,
                Err(_) => {
                    // We probably couldn't find the file, but that's fine.
                    // It would be good to collect this error somewhere.
                    continue;
                }
            };
            let buffer = owned_data.get_data();

            match obj_ref {
                ObjectReference::Regular {
                    path, functions, ..
                } => {
                    let macho_file = File::parse(buffer)
                        .or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;
                    let addresses_in_this_object =
                        translate_addresses_to_object(&path, &macho_file, functions);
                    remainder.extend(collect_debug_info_and_remainder(
                        &macho_file,
                        &addresses_in_this_object,
                        &mut symbolication_result,
                    )?);
                }
                ObjectReference::Archive {
                    path, archive_info, ..
                } => {
                    let archive = goblin::archive::Archive::parse(buffer)?;
                    for (name_in_archive, functions) in archive_info {
                        let buffer = match archive.extract(&name_in_archive, buffer) {
                            Ok(buffer) => buffer,
                            Err(_) => continue,
                        };
                        let macho_file = File::parse(buffer)
                            .or_else(|x| Err(GetSymbolsError::MachOHeaderParseError(x)))?;
                        let addresses_in_this_object =
                            translate_addresses_to_object(&path, &macho_file, functions);
                        remainder.extend(collect_debug_info_and_remainder(
                            &macho_file,
                            &addresses_in_this_object,
                            &mut symbolication_result,
                        )?);
                    }
                }
            };
        }
    }

    Ok(symbolication_result)
}

fn translate_addresses_to_object<'data: 'file, 'file, O>(
    _path: &Path,
    macho_file: &'file O,
    mut functions: HashMap<Vec<u8>, Vec<AddressWithOffset>>,
) -> Vec<AddressPair>
where
    O: object::Object<'data, 'file>,
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

fn collect_debug_info_and_remainder<'data: 'file, 'file, 'a, O, R>(
    macho_file: &'file O,
    addresses: &[AddressPair],
    symbolication_result: &mut R,
) -> Result<Vec<ObjectReference>>
where
    O: object::Object<'data, 'file>,
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

    let mut combined: Vec<_> = Vec::new();
    for (path, archive_info) in archives.into_iter() {
        combined.push(ObjectReference::Archive { path, archive_info });
    }
    for (path, functions) in regular_objects.into_iter() {
        combined.push(ObjectReference::Regular { path, functions });
    }

    Ok(combined)
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
