use crate::dwarf::{collect_dwarf_address_debug_data, make_address_pairs_for_root_object};
use crate::error::{GetSymbolsError, Result};
use crate::path_mapper::PathMapper;
use crate::shared::{
    get_symbolication_result_for_addresses_from_object, object_to_map, FileContents,
    FileContentsWrapper, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use object::{File, FileKind, Object, ObjectSection, ReadRef, SectionKind};
use std::cmp;
use std::io::Cursor;
use uuid::Uuid;

const UUID_SIZE: usize = 16;
const PAGE_SIZE: usize = 4096;

pub fn get_symbolication_result<R>(
    file_kind: FileKind,
    file_contents: FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let elf_file =
        File::parse(&file_contents).map_err(|e| GetSymbolsError::ObjectParseError(file_kind, e))?;

    let elf_id =
        get_elf_id(&elf_file).ok_or(GetSymbolsError::InvalidInputError("id cannot be read"))?;
    let elf_id_string = format!("{:X}0", elf_id.to_simple());
    let SymbolicationQuery { breakpad_id, .. } = query;
    if elf_id_string != breakpad_id {
        return Err(GetSymbolsError::UnmatchedBreakpadId(
            elf_id_string,
            breakpad_id.to_string(),
        ));
    }

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(debugdata) = elf_file.section_by_name(".gnu_debugdata") {
        if let Ok(data) = debugdata.data() {
            let mut cursor = Cursor::new(data);
            let mut objdata = Vec::new();
            if let Ok(()) = lzma_rs::xz_decompress(&mut cursor, &mut objdata) {
                if let Ok(elf_file) = File::parse(&objdata[..]) {
                    let file_contents = FileContentsWrapper::new(&objdata[..]);
                    return get_symbolication_result_impl(elf_file, &file_contents, query);
                }
            }
        }
    }

    get_symbolication_result_impl(elf_file, &file_contents, query)
}

pub fn get_symbolication_result_impl<'data, R>(
    elf_file: File<'data, impl ReadRef<'data>>,
    file_contents: &'data FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery,
) -> Result<R>
where
    R: SymbolicationResult,
{
    let (addresses, mut symbolication_result) = match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            let map = object_to_map(&elf_file);
            return Ok(R::from_full_map(map));
        }
        SymbolicationResultKind::SymbolsForAddresses {
            addresses,
            with_debug_info,
        } => {
            let symbolication_result =
                get_symbolication_result_for_addresses_from_object(addresses, &elf_file);
            if !with_debug_info {
                return Ok(symbolication_result);
            }
            (addresses, symbolication_result)
        }
    };

    let addresses: Vec<_> = make_address_pairs_for_root_object(addresses, &elf_file);
    let mut path_mapper = PathMapper::new();
    collect_dwarf_address_debug_data(
        file_contents.full_range(),
        &elf_file,
        &addresses,
        &mut symbolication_result,
        &mut path_mapper,
    );
    Ok(symbolication_result)
}

fn create_elf_id(identifier: &[u8], little_endian: bool) -> Uuid {
    // Make sure that we have exactly UUID_SIZE bytes available
    let mut data = [0u8; UUID_SIZE];
    let len = cmp::min(identifier.len(), UUID_SIZE);
    data[0..len].copy_from_slice(&identifier[0..len]);

    if little_endian {
        // The file ELF file targets a little endian architecture. Convert to
        // network byte order (big endian) to match the Breakpad processor's
        // expectations. For big endian object files, this is not needed.
        data[0..4].reverse(); // uuid field 1
        data[4..6].reverse(); // uuid field 2
        data[6..8].reverse(); // uuid field 3
    }

    Uuid::from_bytes(data)
}

/// Tries to obtain the object identifier of an ELF object.
///
/// As opposed to Mach-O, ELF does not specify a unique ID for object files in
/// its header. Compilers and linkers usually add either `SHT_NOTE` sections or
/// `PT_NOTE` program header elements for this purpose. If one of these notes
/// is present, ElfFile's build_id() method will find it.
///
/// If neither of the above are present, this function will hash the first page
/// of the `.text` section (program code). This matches what the Breakpad
/// processor does.
///
/// If all of the above fails, this function will return `None`.
pub fn get_elf_id<'data: 'file, 'file>(elf_file: &'file impl Object<'data, 'file>) -> Option<Uuid> {
    if let Some(identifier) = elf_file.build_id().ok()? {
        return Some(create_elf_id(identifier, elf_file.is_little_endian()));
    }

    // We were not able to locate the build ID, so fall back to hashing the
    // first page of the ".text" (program code) section. This algorithm XORs
    // 16-byte chunks directly into a UUID buffer.
    if let Some(section_data) = find_text_section(elf_file) {
        let mut hash = [0; UUID_SIZE];
        for i in 0..cmp::min(section_data.len(), PAGE_SIZE) {
            hash[i % UUID_SIZE] ^= section_data[i];
        }

        return Some(create_elf_id(&hash, elf_file.is_little_endian()));
    }

    None
}

/// Returns a reference to the data of the the .text section in an ELF binary.
fn find_text_section<'data: 'file, 'file>(
    file: &'file impl Object<'data, 'file>,
) -> Option<&'data [u8]> {
    file.sections()
        .find(|header| header.kind() == SectionKind::Text)
        .and_then(|header| header.data().ok())
}
