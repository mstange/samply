use crate::error::Error;
use crate::shared::{BasePath, FileContents, FileContentsWrapper};
use crate::symbol_map::{
    GenericSymbolMap, SymbolMap, SymbolMapDataMidTrait, SymbolMapDataOuterTrait,
};
use crate::symbol_map_object::{FunctionAddressesComputer, ObjectSymbolMapDataMid};
use crate::{debug_id_for_object, ElfBuildId, FileAndPathHelper, FileLocation};
use debugid::DebugId;
use gimli::{CieOrFde, EhFrame, UnwindSection};
use object::{File, FileKind, Object, ObjectSection, ReadRef};
use std::io::Cursor;
use std::path::Path;

pub async fn load_symbol_map_for_elf<'h, T, H>(
    file_contents: FileContentsWrapper<T>,
    file_kind: FileKind,
    base_path: &BasePath,
    helper: &'h H,
) -> Result<SymbolMap, Error>
where
    T: FileContents + 'static,
    H: FileAndPathHelper<'h, F = T>,
{
    let elf_file =
        File::parse(&file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    if let Some(symbol_map) =
        try_to_get_symbol_map_from_debug_link(&elf_file, file_kind, helper).await
    {
        return Ok(symbol_map);
    }

    if let Some(supplementary_file) =
        try_to_load_supplementary_file(&elf_file, base_path, helper).await
    {
        let owner = ElfSymbolMapData::new(file_contents, Some(supplementary_file), file_kind, None);
        let symbol_map = GenericSymbolMap::new(owner, base_path)?;
        return Ok(SymbolMap(Box::new(symbol_map)));
    }

    // If this file has a .gnu_debugdata section, use the uncompressed object from that section instead.
    if let Some(symbol_map) =
        try_get_symbol_map_from_mini_debug_info(&elf_file, file_kind, base_path)
    {
        return Ok(symbol_map);
    }

    let owner = ElfSymbolMapData::new(file_contents, None, file_kind, None);
    let symbol_map = GenericSymbolMap::new(owner, base_path)?;
    Ok(SymbolMap(Box::new(symbol_map)))
}

async fn try_to_get_symbol_map_from_debug_link<'h, 'data, H, R>(
    elf_file: &File<'data, R>,
    file_kind: FileKind,
    helper: &'h H,
) -> Option<SymbolMap>
where
    H: FileAndPathHelper<'h>,
    R: ReadRef<'data>,
{
    let (name, crc) = elf_file.gnu_debuglink().ok().flatten()?;
    let debug_id = debug_id_for_object(elf_file)?;
    let name = std::str::from_utf8(name).ok()?;
    let candidate_paths = helper
        .get_candidate_paths_for_gnu_debug_link_dest(name)
        .ok()?;

    for candidate_path in candidate_paths {
        let symbol_map = get_symbol_map_for_debug_link_candidate(
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

async fn get_symbol_map_for_debug_link_candidate<'h, H>(
    path: &Path,
    debug_id: DebugId,
    expected_crc: u32,
    file_kind: FileKind,
    helper: &'h H,
) -> Result<SymbolMap, Error>
where
    H: FileAndPathHelper<'h>,
{
    let file_location = FileLocation::Path(path.to_owned());
    let file_contents = helper
        .open_file(&file_location)
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(path.to_string_lossy().to_string(), e))?;
    let file_contents = FileContentsWrapper::new(file_contents);
    let actual_crc = compute_debug_link_crc_of_file_contents(&file_contents)?;

    if actual_crc != expected_crc {
        return Err(Error::DebugLinkCrcMismatch(actual_crc, expected_crc));
    }

    let base_path = file_location.to_base_path();
    let owner = ElfSymbolMapData::new(file_contents, None, file_kind, Some(debug_id));
    let symbol_map = GenericSymbolMap::new(owner, &base_path)?;
    Ok(SymbolMap(Box::new(symbol_map)))
}

// https://www-zeuthen.desy.de/unix/unixguide/infohtml/gdb/Separate-Debug-Files.html
struct GnuDebugLinkCrc32Computer(pub u32);

impl GnuDebugLinkCrc32Computer {
    pub fn consume(&mut self, buf: &[u8]) {
        const CRC32_TABLE: [u32; 256] = [
            0x00000000, 0x77073096, 0xee0e612c, 0x990951ba, 0x076dc419, 0x706af48f, 0xe963a535,
            0x9e6495a3, 0x0edb8832, 0x79dcb8a4, 0xe0d5e91e, 0x97d2d988, 0x09b64c2b, 0x7eb17cbd,
            0xe7b82d07, 0x90bf1d91, 0x1db71064, 0x6ab020f2, 0xf3b97148, 0x84be41de, 0x1adad47d,
            0x6ddde4eb, 0xf4d4b551, 0x83d385c7, 0x136c9856, 0x646ba8c0, 0xfd62f97a, 0x8a65c9ec,
            0x14015c4f, 0x63066cd9, 0xfa0f3d63, 0x8d080df5, 0x3b6e20c8, 0x4c69105e, 0xd56041e4,
            0xa2677172, 0x3c03e4d1, 0x4b04d447, 0xd20d85fd, 0xa50ab56b, 0x35b5a8fa, 0x42b2986c,
            0xdbbbc9d6, 0xacbcf940, 0x32d86ce3, 0x45df5c75, 0xdcd60dcf, 0xabd13d59, 0x26d930ac,
            0x51de003a, 0xc8d75180, 0xbfd06116, 0x21b4f4b5, 0x56b3c423, 0xcfba9599, 0xb8bda50f,
            0x2802b89e, 0x5f058808, 0xc60cd9b2, 0xb10be924, 0x2f6f7c87, 0x58684c11, 0xc1611dab,
            0xb6662d3d, 0x76dc4190, 0x01db7106, 0x98d220bc, 0xefd5102a, 0x71b18589, 0x06b6b51f,
            0x9fbfe4a5, 0xe8b8d433, 0x7807c9a2, 0x0f00f934, 0x9609a88e, 0xe10e9818, 0x7f6a0dbb,
            0x086d3d2d, 0x91646c97, 0xe6635c01, 0x6b6b51f4, 0x1c6c6162, 0x856530d8, 0xf262004e,
            0x6c0695ed, 0x1b01a57b, 0x8208f4c1, 0xf50fc457, 0x65b0d9c6, 0x12b7e950, 0x8bbeb8ea,
            0xfcb9887c, 0x62dd1ddf, 0x15da2d49, 0x8cd37cf3, 0xfbd44c65, 0x4db26158, 0x3ab551ce,
            0xa3bc0074, 0xd4bb30e2, 0x4adfa541, 0x3dd895d7, 0xa4d1c46d, 0xd3d6f4fb, 0x4369e96a,
            0x346ed9fc, 0xad678846, 0xda60b8d0, 0x44042d73, 0x33031de5, 0xaa0a4c5f, 0xdd0d7cc9,
            0x5005713c, 0x270241aa, 0xbe0b1010, 0xc90c2086, 0x5768b525, 0x206f85b3, 0xb966d409,
            0xce61e49f, 0x5edef90e, 0x29d9c998, 0xb0d09822, 0xc7d7a8b4, 0x59b33d17, 0x2eb40d81,
            0xb7bd5c3b, 0xc0ba6cad, 0xedb88320, 0x9abfb3b6, 0x03b6e20c, 0x74b1d29a, 0xead54739,
            0x9dd277af, 0x04db2615, 0x73dc1683, 0xe3630b12, 0x94643b84, 0x0d6d6a3e, 0x7a6a5aa8,
            0xe40ecf0b, 0x9309ff9d, 0x0a00ae27, 0x7d079eb1, 0xf00f9344, 0x8708a3d2, 0x1e01f268,
            0x6906c2fe, 0xf762575d, 0x806567cb, 0x196c3671, 0x6e6b06e7, 0xfed41b76, 0x89d32be0,
            0x10da7a5a, 0x67dd4acc, 0xf9b9df6f, 0x8ebeeff9, 0x17b7be43, 0x60b08ed5, 0xd6d6a3e8,
            0xa1d1937e, 0x38d8c2c4, 0x4fdff252, 0xd1bb67f1, 0xa6bc5767, 0x3fb506dd, 0x48b2364b,
            0xd80d2bda, 0xaf0a1b4c, 0x36034af6, 0x41047a60, 0xdf60efc3, 0xa867df55, 0x316e8eef,
            0x4669be79, 0xcb61b38c, 0xbc66831a, 0x256fd2a0, 0x5268e236, 0xcc0c7795, 0xbb0b4703,
            0x220216b9, 0x5505262f, 0xc5ba3bbe, 0xb2bd0b28, 0x2bb45a92, 0x5cb36a04, 0xc2d7ffa7,
            0xb5d0cf31, 0x2cd99e8b, 0x5bdeae1d, 0x9b64c2b0, 0xec63f226, 0x756aa39c, 0x026d930a,
            0x9c0906a9, 0xeb0e363f, 0x72076785, 0x05005713, 0x95bf4a82, 0xe2b87a14, 0x7bb12bae,
            0x0cb61b38, 0x92d28e9b, 0xe5d5be0d, 0x7cdcefb7, 0x0bdbdf21, 0x86d3d2d4, 0xf1d4e242,
            0x68ddb3f8, 0x1fda836e, 0x81be16cd, 0xf6b9265b, 0x6fb077e1, 0x18b74777, 0x88085ae6,
            0xff0f6a70, 0x66063bca, 0x11010b5c, 0x8f659eff, 0xf862ae69, 0x616bffd3, 0x166ccf45,
            0xa00ae278, 0xd70dd2ee, 0x4e048354, 0x3903b3c2, 0xa7672661, 0xd06016f7, 0x4969474d,
            0x3e6e77db, 0xaed16a4a, 0xd9d65adc, 0x40df0b66, 0x37d83bf0, 0xa9bcae53, 0xdebb9ec5,
            0x47b2cf7f, 0x30b5ffe9, 0xbdbdf21c, 0xcabac28a, 0x53b39330, 0x24b4a3a6, 0xbad03605,
            0xcdd70693, 0x54de5729, 0x23d967bf, 0xb3667a2e, 0xc4614ab8, 0x5d681b02, 0x2a6f2b94,
            0xb40bbe37, 0xc30c8ea1, 0x5a05df1b, 0x2d02ef8d,
        ];

        let mut crc = !self.0;
        for byte in buf {
            crc = CRC32_TABLE[(crc as u8 ^ *byte) as usize] ^ (crc >> 8);
        }
        self.0 = !crc;
    }
}

#[test]
fn test_crc() {
    fn gnu_debuglink_crc32(initial: u32, buf: &[u8]) -> u32 {
        let mut computer = GnuDebugLinkCrc32Computer(initial);
        computer.consume(buf);
        computer.0
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
    let mut computer = GnuDebugLinkCrc32Computer(0);

    const CHUNK_SIZE: u64 = 1024 * 1024; // 1MB
    let mut buffer = Vec::with_capacity(CHUNK_SIZE as usize);

    let len = data.len();
    let mut offset = 0;
    while offset < len {
        let chunk_len = CHUNK_SIZE.min(len - offset);
        data.read_bytes_into(&mut buffer, offset, chunk_len as usize)
            .map_err(|e| Error::HelperErrorDuringFileReading("DebugLinkForCrc".to_string(), e))?;
        computer.consume(&buffer);
        buffer.clear();
        offset += CHUNK_SIZE;
    }
    Ok(computer.0)
}

async fn try_to_load_supplementary_file<'h, 'data, H, F, R>(
    elf_file: &File<'data, R>,
    base_path: &BasePath,
    helper: &'h H,
) -> Option<FileContentsWrapper<F>>
where
    H: FileAndPathHelper<'h, F = F>,
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
        .get_candidate_paths_for_supplementary_debug_file(base_path, &path, &supplementary_build_id)
        .ok()?;

    for candidate_path in candidate_paths {
        if let Ok(file_contents) = helper.open_file(&candidate_path).await {
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

fn try_get_symbol_map_from_mini_debug_info<'data, R: ReadRef<'data>>(
    elf_file: &File<'data, R>,
    file_kind: FileKind,
    base_path: &BasePath,
) -> Option<SymbolMap> {
    let debugdata = elf_file.section_by_name(".gnu_debugdata")?;
    let data = debugdata.data().ok()?;
    let mut cursor = Cursor::new(data);
    let mut objdata = Vec::new();
    lzma_rs::xz_decompress(&mut cursor, &mut objdata).ok()?;
    let file_contents = FileContentsWrapper::new(objdata);
    let owner = ElfSymbolMapData::new(file_contents, None, file_kind, None);
    let symbol_map = GenericSymbolMap::new(owner, base_path).ok()?;
    Some(SymbolMap(Box::new(symbol_map)))
}

struct ElfSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
    supplementary_file_data: Option<FileContentsWrapper<T>>,
    file_kind: FileKind,
    override_debug_id: Option<DebugId>,
}

impl<T: FileContents> ElfSymbolMapData<T> {
    pub fn new(
        file_data: FileContentsWrapper<T>,
        supplementary_file_data: Option<FileContentsWrapper<T>>,
        file_kind: FileKind,
        override_debug_id: Option<DebugId>,
    ) -> Self {
        Self {
            file_data,
            supplementary_file_data,
            file_kind,
            override_debug_id,
        }
    }
}

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for ElfSymbolMapData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let object =
            File::parse(&self.file_data).map_err(|e| Error::ObjectParseError(self.file_kind, e))?;
        let supplementary_object = match self.supplementary_file_data.as_ref() {
            Some(supplementary_file_data) => Some(
                File::parse(supplementary_file_data)
                    .map_err(|e| Error::ObjectParseError(self.file_kind, e))?,
            ),
            None => None,
        };
        let debug_id = if let Some(debug_id) = self.override_debug_id {
            debug_id
        } else {
            debug_id_for_object(&object)
                .ok_or(Error::InvalidInputError("debug ID cannot be read"))?
        };
        let object = ObjectSymbolMapDataMid::new(
            object,
            supplementary_object,
            ElfFunctionAddressesComputer,
            &self.file_data,
            self.supplementary_file_data.as_ref(),
            None,
            debug_id,
        );

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
