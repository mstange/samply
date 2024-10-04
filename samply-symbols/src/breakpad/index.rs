use std::fmt::Debug;
use std::str::FromStr;
use std::{mem, str};

use debugid::DebugId;
use memchr::memchr;
use nom::bytes::complete::{tag, take_while};
use nom::character::complete::{hex_digit1, space1};
use nom::combinator::{cut, map_res, opt, rest};
use nom::error::{Error, ErrorKind, ParseError};
use nom::multi::separated_list1;
use nom::sequence::{terminated, tuple};
use nom::{Err, IResult};
use zerocopy::{AsBytes, LittleEndian, Ref, U32, U64};
use zerocopy_derive::{FromBytes, FromZeroes, Unaligned};

use crate::CodeId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpadIndex {
    pub module_info_bytes: Vec<u8>,
    pub debug_name: String,
    pub debug_id: DebugId,
    pub arch: String,
    pub os: String,
    pub name: Option<String>,
    pub code_id: Option<CodeId>,
    pub symbol_addresses: Vec<u32>,
    pub symbol_offsets: Vec<BreakpadSymbolType>,
    pub files: ItemMap<BreakpadFileLine>,
    pub inline_origins: ItemMap<BreakpadInlineOriginLine>,
}

const HEADER_SIZE: u32 = std::mem::size_of::<BreakpadSymindexFileHeader>() as u32;
const FILE_OR_INLINE_ORIGIN_ENTRY_SIZE: u32 = std::mem::size_of::<FileOrInlineOriginEntry>() as u32;
const SYMBOL_ADDRESS_SIZE: u32 = std::mem::size_of::<u32>() as u32;
const SYMBOL_ENTRY_SIZE: u32 = std::mem::size_of::<SymbolEntry>() as u32;

impl BreakpadIndex {
    pub fn parse_symindex_file(data: &[u8]) -> Result<BreakpadIndex, BreakpadSymindexParseError> {
        let header_bytes = data
            .get(..HEADER_SIZE as usize)
            .ok_or(BreakpadSymindexParseError::FileTooSmallForHeader)?;
        let header = Ref::<&[u8], BreakpadSymindexFileHeader>::new_unaligned(header_bytes).unwrap();
        if &header.magic != b"SYMINDEX" {
            return Err(BreakpadSymindexParseError::WrongMagicBytes);
        }
        let module_info_end_offset = header
            .module_info_offset
            .get()
            .checked_add(header.module_info_len.get())
            .ok_or(BreakpadSymindexParseError::ModuleInfoOffsetLenOverflow)?;
        let module_info_bytes = data
            .get(header.module_info_offset.get() as usize..module_info_end_offset as usize)
            .ok_or(BreakpadSymindexParseError::CouldntReadModuleInfoBytes)?;

        let (debug_id, os, arch, debug_name, name, code_id) = {
            let mut module_info = None;
            let mut code_id = None;
            let mut name = None;
            let mut module_info_line_buffer = LineBuffer::default();
            module_info_line_buffer.consume(module_info_bytes, |_offset, line_slice| {
                // Every file must start with a "MODULE " line.
                if let Ok((_r, (os, arch, debug_id, debug_name))) = module_line(line_slice) {
                    module_info = Some((
                        debug_id,
                        os.to_string(),
                        arch.to_string(),
                        debug_name.to_string(),
                    ));
                } else if let Ok((_r, (code_id_str, name_str))) = info_code_id_line(line_slice) {
                    code_id = CodeId::from_str(code_id_str).ok();
                    name = name_str.map(ToOwned::to_owned);
                }
            });
            module_info_line_buffer.finish(|_offset, line_slice| {
                // Every file must start with a "MODULE " line.
                if let Ok((_r, (os, arch, debug_id, debug_name))) = module_line(line_slice) {
                    module_info = Some((
                        debug_id,
                        os.to_string(),
                        arch.to_string(),
                        debug_name.to_string(),
                    ));
                } else if let Ok((_r, (code_id_str, name_str))) = info_code_id_line(line_slice) {
                    code_id = CodeId::from_str(code_id_str).ok();
                    name = name_str.map(ToOwned::to_owned);
                }
            });
            match module_info {
                Some((debug_id, os, arch, debug_name)) => {
                    (debug_id, os, arch, debug_name, name, code_id)
                }
                None => return Err(BreakpadSymindexParseError::CouldntParseModuleInfoLine),
            }
        };
        let file_list_bytes_len = header
            .file_count
            .get()
            .checked_mul(FILE_OR_INLINE_ORIGIN_ENTRY_SIZE)
            .ok_or(BreakpadSymindexParseError::FileListByteLenOverflow)?;
        let file_list_end_offset = header
            .file_entries_offset
            .get()
            .checked_add(file_list_bytes_len)
            .ok_or(BreakpadSymindexParseError::FileListOffsetLenOverflow)?;
        let file_list_bytes = data
            .get(header.file_entries_offset.get() as usize..file_list_end_offset as usize)
            .ok_or(BreakpadSymindexParseError::CouldntReadFileListBytes)?;
        let file_list =
            Ref::<&[u8], [FileOrInlineOriginEntry]>::new_slice_unaligned(file_list_bytes).unwrap();
        let inline_origin_list_bytes_len = header
            .inline_origin_count
            .get()
            .checked_mul(FILE_OR_INLINE_ORIGIN_ENTRY_SIZE)
            .ok_or(BreakpadSymindexParseError::InlineOriginListByteLenOverflow)?;
        let inline_origin_list_end_offset = header
            .inline_origin_entries_offset
            .get()
            .checked_add(inline_origin_list_bytes_len)
            .ok_or(BreakpadSymindexParseError::InlineOriginListOffsetLenOverflow)?;
        let inline_origin_list_bytes = data
            .get(
                header.inline_origin_entries_offset.get() as usize
                    ..inline_origin_list_end_offset as usize,
            )
            .ok_or(BreakpadSymindexParseError::CouldntReadInlineOriginListBytes)?;
        let inline_origin_list =
            Ref::<&[u8], [FileOrInlineOriginEntry]>::new_slice_unaligned(inline_origin_list_bytes)
                .unwrap();
        let symbol_address_list_bytes_len = header
            .symbol_count
            .get()
            .checked_mul(SYMBOL_ADDRESS_SIZE)
            .ok_or(BreakpadSymindexParseError::SymbolAddressListByteLenOverflow)?;
        let symbol_address_list_end_offset = header
            .symbol_addresses_offset
            .get()
            .checked_add(symbol_address_list_bytes_len)
            .ok_or(BreakpadSymindexParseError::SymbolAddressListOffsetLenOverflow)?;
        let symbol_address_list_bytes = data
            .get(
                header.symbol_addresses_offset.get() as usize
                    ..symbol_address_list_end_offset as usize,
            )
            .ok_or(BreakpadSymindexParseError::CouldntReadSymbolAddressListBytes)?;
        let symbol_address_list =
            Ref::<&[u8], [U32<LittleEndian>]>::new_slice_unaligned(symbol_address_list_bytes)
                .unwrap();
        let symbol_entry_list_bytes_len = header
            .symbol_count
            .get()
            .checked_mul(SYMBOL_ENTRY_SIZE)
            .ok_or(BreakpadSymindexParseError::SymbolEntryListByteLenOverflow)?;
        let symbol_entry_list_end_offset = header
            .symbol_entries_offset
            .get()
            .checked_add(symbol_entry_list_bytes_len)
            .ok_or(BreakpadSymindexParseError::SymbolEntryListOffsetLenOverflow)?;
        let symbol_entry_list_bytes = data
            .get(header.symbol_entries_offset.get() as usize..symbol_entry_list_end_offset as usize)
            .ok_or(BreakpadSymindexParseError::CouldntReadSymbolEntryListBytes)?;
        let symbol_entry_list =
            Ref::<&[u8], [SymbolEntry]>::new_slice_unaligned(symbol_entry_list_bytes).unwrap();

        let files: Vec<BreakpadFileLine> = file_list
            .into_slice()
            .iter()
            .map(|entry| BreakpadFileLine {
                index: entry.index.get(),
                file_offset: entry.offset.get(),
                line_length: entry.line_len.get(),
            })
            .collect();
        let inline_origins: Vec<BreakpadInlineOriginLine> = inline_origin_list
            .into_slice()
            .iter()
            .map(|entry| BreakpadInlineOriginLine {
                index: entry.index.get(),
                file_offset: entry.offset.get(),
                line_length: entry.line_len.get(),
            })
            .collect();
        let symbol_addresses: Vec<u32> = symbol_address_list.iter().map(|a| a.get()).collect();
        let symbol_offsets: Vec<BreakpadSymbolType> = symbol_entry_list
            .into_slice()
            .iter()
            .map(|entry| {
                if entry.kind.get() == SYMBOL_ENTRY_KIND_PUBLIC {
                    // PUBLIC
                    BreakpadSymbolType::Public(BreakpadPublicSymbol {
                        file_offset: entry.offset.get(),
                        line_length: entry.line_or_block_len.get(),
                    })
                } else {
                    // FUNC
                    BreakpadSymbolType::Func(BreakpadFuncSymbol {
                        file_offset: entry.offset.get(),
                        block_length: entry.line_or_block_len.get(),
                    })
                }
            })
            .collect();
        Ok(BreakpadIndex {
            module_info_bytes: module_info_bytes.to_owned(),
            debug_name,
            debug_id,
            arch,
            os,
            name,
            code_id,
            symbol_addresses,
            symbol_offsets,
            files: ItemMap::from_sorted_vec(files),
            inline_origins: ItemMap::from_sorted_vec(inline_origins),
        })
    }

    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        let header_len = HEADER_SIZE;
        let module_info_offset = header_len;
        let module_info_len = self.module_info_bytes.len() as u32;
        let padding_after_module_info = align_to_4_bytes(module_info_len) - module_info_len;
        let file_entries_offset = module_info_offset + module_info_len + padding_after_module_info;
        let file_count = self.files.len() as u32;
        let file_entries_len = file_count * FILE_OR_INLINE_ORIGIN_ENTRY_SIZE;
        let inline_origin_entries_offset = file_entries_offset + file_entries_len;
        let inline_origin_count = self.inline_origins.len() as u32;
        let inline_origin_entries_len = inline_origin_count * FILE_OR_INLINE_ORIGIN_ENTRY_SIZE;
        let symbol_addresses_offset = inline_origin_entries_offset + inline_origin_entries_len;
        let symbol_count = self.symbol_addresses.len() as u32;
        let symbol_addresses_len = symbol_count * SYMBOL_ADDRESS_SIZE;
        let symbol_entries_offset = symbol_addresses_offset + symbol_addresses_len;
        let symbol_entries_len = symbol_count * SYMBOL_ENTRY_SIZE;
        let total_file_len = symbol_entries_offset + symbol_entries_len;
        let header = BreakpadSymindexFileHeader {
            magic: *b"SYMINDEX",
            version: 1.into(),
            module_info_offset: module_info_offset.into(),
            module_info_len: module_info_len.into(),
            file_count: file_count.into(),
            file_entries_offset: file_entries_offset.into(),
            inline_origin_count: inline_origin_count.into(),
            inline_origin_entries_offset: inline_origin_entries_offset.into(),
            symbol_count: symbol_count.into(),
            symbol_addresses_offset: symbol_addresses_offset.into(),
            symbol_entries_offset: symbol_entries_offset.into(),
        };

        let mut vec = Vec::with_capacity(total_file_len as usize);
        vec.extend_from_slice(header.as_bytes());
        vec.extend_from_slice(&self.module_info_bytes);
        vec.extend(std::iter::repeat(0).take(padding_after_module_info as usize));
        for file in self.files.as_slice() {
            vec.extend_from_slice(
                FileOrInlineOriginEntry {
                    index: file.index.into(),
                    line_len: file.line_length.into(),
                    offset: file.file_offset.into(),
                }
                .as_bytes(),
            );
        }
        for inline_origin in self.inline_origins.as_slice() {
            vec.extend_from_slice(
                FileOrInlineOriginEntry {
                    index: inline_origin.index.into(),
                    line_len: inline_origin.line_length.into(),
                    offset: inline_origin.file_offset.into(),
                }
                .as_bytes(),
            );
        }
        for symbol_address in &self.symbol_addresses {
            vec.extend_from_slice(U32::<LittleEndian>::from(*symbol_address).as_bytes());
        }
        for symbol in &self.symbol_offsets {
            match symbol {
                BreakpadSymbolType::Public(public) => {
                    vec.extend_from_slice(
                        SymbolEntry {
                            kind: SYMBOL_ENTRY_KIND_PUBLIC.into(),
                            line_or_block_len: public.line_length.into(),
                            offset: public.file_offset.into(),
                        }
                        .as_bytes(),
                    );
                }
                BreakpadSymbolType::Func(func) => {
                    vec.extend_from_slice(
                        SymbolEntry {
                            kind: SYMBOL_ENTRY_KIND_FUNC.into(),
                            line_or_block_len: func.block_length.into(),
                            offset: func.file_offset.into(),
                        }
                        .as_bytes(),
                    );
                }
            }
        }

        assert_eq!(vec.len(), total_file_len as usize);

        vec
    }
}

#[inline]
fn round_up_to_multiple(value: u32, factor: u32) -> u32 {
    (value + factor - 1) / factor * factor
}

fn align_to_4_bytes(value: u32) -> u32 {
    round_up_to_multiple(value, 4)
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BreakpadSymindexParseError {
    #[error("Not enough bytes in the file for the file header")]
    FileTooSmallForHeader,

    #[error("Wrong magic bytes in the symindex header")]
    WrongMagicBytes,

    #[error("Module offset + len overflowed u32")]
    ModuleInfoOffsetLenOverflow,

    #[error("Module info bytes couldn't be read from the file")]
    CouldntReadModuleInfoBytes,

    #[error("Module info bytes couldn't be parsed as utf-8")]
    ModuleInfoNotUtf8,

    #[error("MODULE INFO couldn't be parsed in module info section")]
    CouldntParseModuleInfoLine,

    #[error("File count * file entry size overflowed")]
    FileListByteLenOverflow,

    #[error("File list offset + len overflowed u32")]
    FileListOffsetLenOverflow,

    #[error("File list bytes couldn't be read from the file")]
    CouldntReadFileListBytes,

    #[error("Inline origin count * inline origin entry size overflowed")]
    InlineOriginListByteLenOverflow,

    #[error("Inline origin offset + len overflowed u32")]
    InlineOriginListOffsetLenOverflow,

    #[error("InlineOrigin list bytes couldn't be read from the file")]
    CouldntReadInlineOriginListBytes,

    #[error("Symbol count * 4 bytes per address overflowed")]
    SymbolAddressListByteLenOverflow,

    #[error("Symbol address list offset + len overflowed u32")]
    SymbolAddressListOffsetLenOverflow,

    #[error("Symbol address list bytes couldn't be read from the file")]
    CouldntReadSymbolAddressListBytes,

    #[error("Symbol count * symbol entry size overflowed")]
    SymbolEntryListByteLenOverflow,

    #[error("Symbol entry list offset + len overflowed u32")]
    SymbolEntryListOffsetLenOverflow,

    #[error("Symbol entry list bytes couldn't be read from the file")]
    CouldntReadSymbolEntryListBytes,
}

#[derive(FromZeroes, FromBytes, zerocopy_derive::AsBytes, Unaligned)]
#[repr(C)]
struct BreakpadSymindexFileHeader {
    /// Always b"SYMINDEX", at 0
    magic: [u8; 8],
    /// Always 1, at 8
    version: U32<LittleEndian>,
    /// Points right after header, to where the module info starts, 4-byte aligned, at 12
    module_info_offset: U32<LittleEndian>,
    /// The length, in bytes, of the module info, at 16
    module_info_len: U32<LittleEndian>,
    /// The number of entries in the file list, at 20
    file_count: U32<LittleEndian>,
    /// Points to the start of the file list, 4-byte aligned, at 24
    file_entries_offset: U32<LittleEndian>,
    /// The number of entries in the inline origin list, at 28
    inline_origin_count: U32<LittleEndian>,
    /// Poinst to the start of the inline origin list, 4-byte aligned, at 32
    inline_origin_entries_offset: U32<LittleEndian>,
    /// The number of symbols, at 36
    symbol_count: U32<LittleEndian>,
    /// Points to the start of the symbol address list, 4-byte aligned, at 40
    symbol_addresses_offset: U32<LittleEndian>,
    /// Points to the start of the symbol entry list, 4-byte aligned, at 44
    symbol_entries_offset: U32<LittleEndian>,
}

#[derive(FromZeroes, FromBytes, zerocopy_derive::AsBytes, Unaligned)]
#[repr(C)]
struct FileOrInlineOriginEntry {
    pub index: U32<LittleEndian>,
    pub line_len: U32<LittleEndian>,
    pub offset: U64<LittleEndian>,
}

const SYMBOL_ENTRY_KIND_PUBLIC: u32 = 0;
const SYMBOL_ENTRY_KIND_FUNC: u32 = 1;

#[derive(FromZeroes, FromBytes, zerocopy_derive::AsBytes, Unaligned)]
#[repr(C)]
struct SymbolEntry {
    /// Uses `SYMBOL_ENTRY_KIND_*` constants (0 for PUBLIC, 1 for FUNC)
    pub kind: U32<LittleEndian>,
    /// For PUBLIC: The length of the line, starting at PUBLIC and ending before the line break. For FUNC: The length of the func block, starting at the FUNC and ending at the next top-level sym entry (symbol, stack record) or file end
    pub line_or_block_len: U32<LittleEndian>,
    /// File offset of the PUBLIC / FUNC string.
    pub offset: U64<LittleEndian>,
}

/// File extension: .symindex
///
/// Format: (all numbers encoded as little-endian)
///
/// magic: [u8; 8], // always b"SYMINDEX", at 0
/// version: u32, // always 1, at 8
/// module_info_offset: u32, // points right after header, to where the module info starts, 4-byte aligned, at 12
/// module_info_len: u32, // the length, in bytes, of the module info, at 16
/// file_count: u32, // the number of entries in the file list, at 20
/// file_entries_offset: u32, // points to the start of the file list, 4-byte aligned, at 24
/// inline_origin_count: u32, // the number of entries in the inline origin list, at 28
/// inline_origin_entries_offset: u32, // poinst to the start of the inline origin list, 4-byte aligned, at 32
/// symbol_count: u32, // the number of symbols, at 36
/// symbol_addresses_offset: u32, // points to the start of the symbol address list, 4-byte aligned, at 40
/// symbol_entries_offset: u32, // points to the start of the symbol entry list, 4-byte aligned, at 44
///
/// /// Module info: utf-8 encoded string, contains line breaks, and the lines start with MODULE and INFO
/// module_info: [u8; module_info_len], // located at module_info_offset
///
/// /// File list:
/// file_list: [FileOrInlineOriginEntry; file_count], // located at file_entries_offset
///
/// /// Inline list:
/// inline_origin_list: [FileOrInlineOriginEntry; inline_origin_count], // located at file_entries_offset
///
/// /// Symbol addresses:
/// symbol_addresses: [u32; symbol_count], // located at symbol_addresses_offset
///
/// /// Symbol entries:
/// symbol_entries: [SymbolEntry; symbol_count], // located at symbol_entries_offset
///
/// #[repr(C)]
/// struct FileOrInlineOriginEntry {
///   pub index: u32,
///   pub line_len: u32,
///   pub offset: u64,
/// }
///
/// #[repr(C)]
/// struct SymbolEntry {
///   pub kind: u32, // 0 or 1, 0 meaning Public and 1 meaning Func
///   pub line_or_block_len: u32, // For PUBLIC: The length of the line, starting at PUBLIC and ending before the line break. For FUNC: The length of the func block, starting at the FUNC and ending at the next top-level sym entry (symbol, stack record) or file end
///   pub offset: u64, // File offset of the PUBLIC / FUNC string.
/// }

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BreakpadSymbolType {
    Public(BreakpadPublicSymbol),
    Func(BreakpadFuncSymbol),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadPublicSymbol {
    /// The file offset at which there is the string `PUBLIC ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). PUBLIC symbols only occupy a single line.
    pub line_length: u32,
}

impl BreakpadPublicSymbol {
    pub fn parse<'a>(
        &self,
        input: &'a [u8],
    ) -> Result<BreakpadPublicSymbolInfo<'a>, BreakpadParseError> {
        let (_rest, (_address, name)) =
            public_line(input).map_err(|_| BreakpadParseError::ParsingPublic)?;
        Ok(BreakpadPublicSymbolInfo {
            name: str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)?,
        })
    }
}

/// Returns the first line, excluding trailing `\r*\n`.
///
/// Advances the input to just after `\n`.
fn read_line_and_advance<'a>(input: &mut &'a [u8]) -> &'a [u8] {
    let mut line = if let Some(line_break) = memchr(b'\n', input) {
        let line = &input[..line_break];
        *input = &input[(line_break + 1)..];
        line
    } else {
        let line = *input;
        *input = &[];
        line
    };
    while line.last() == Some(&b'\r') {
        line = &line[..(line.len() - 1)];
    }
    line
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadFuncSymbol {
    /// The file offset at which there is the string `FUNC ` at the start of the line
    pub file_offset: u64,
    /// The number of bytes in the file taken up by this `FUNC` block, including its line record lines.
    pub block_length: u32,
}

impl BreakpadFuncSymbol {
    pub fn parse<'a>(
        &self,
        mut input: &'a [u8],
    ) -> Result<BreakpadFuncSymbolInfo<'a>, BreakpadParseError> {
        let first_line = read_line_and_advance(&mut input);
        let (_rest, (_address, size, name)) =
            func_line(first_line).map_err(|_| BreakpadParseError::ParsingFunc)?;
        let mut inlinees = Vec::new();
        let mut lines = Vec::new();
        while !input.is_empty() {
            let line = read_line_and_advance(&mut input);
            if line.starts_with(b"INLINE ") {
                let (_rest, new_inlinees) =
                    inline_line(line).map_err(|_| BreakpadParseError::ParsingInline)?;
                inlinees.extend(new_inlinees);
            } else if let Ok((_rest, line_data)) = func_line_data(line) {
                lines.push(line_data);
            }
        }
        inlinees.sort_by_key(|inlinee| (inlinee.depth, inlinee.address));
        Ok(BreakpadFuncSymbolInfo {
            name: str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)?,
            size,
            lines,
            inlinees,
        })
    }
}

pub trait FileOrInlineOrigin {
    fn index(&self) -> u32;
    fn offset_and_length(&self) -> (u64, u32);
    fn parse(line: &[u8]) -> Result<&str, BreakpadParseError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMap<I: FileOrInlineOrigin> {
    inner: Vec<I>,
}

impl<I: FileOrInlineOrigin> ItemMap<I> {
    pub fn from_sorted_vec(vec: Vec<I>) -> Self {
        Self { inner: vec }
    }
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub fn as_slice(&self) -> &[I] {
        &self.inner
    }
    pub fn get(&self, index: u32) -> Option<&I> {
        Some(&self.inner[self.get_vec_index(index)?])
    }
    fn get_vec_index(&self, index: u32) -> Option<usize> {
        self.inner.binary_search_by_key(&index, I::index).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadFileLine {
    /// The file index of this file.
    pub index: u32,
    /// The file offset at which there is the string `FILE ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). `FILE` symbols only occupy a single line.
    pub line_length: u32,
}

impl FileOrInlineOrigin for BreakpadFileLine {
    fn index(&self) -> u32 {
        self.index
    }
    fn offset_and_length(&self) -> (u64, u32) {
        (self.file_offset, self.line_length)
    }
    fn parse(input: &[u8]) -> Result<&str, BreakpadParseError> {
        let (_rest, (_index, name)) =
            file_line(input).map_err(|_| BreakpadParseError::ParsingFile)?;
        str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadInlineOriginLine {
    /// The inline origin index of this inline origin.
    pub index: u32,
    /// The file offset at which there is the string `INLINE_ORIGIN ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). `INLINE_ORIGIN` symbols only occupy a single line.
    pub line_length: u32,
}

impl FileOrInlineOrigin for BreakpadInlineOriginLine {
    fn index(&self) -> u32 {
        self.index
    }
    fn offset_and_length(&self) -> (u64, u32) {
        (self.file_offset, self.line_length)
    }
    fn parse(input: &[u8]) -> Result<&str, BreakpadParseError> {
        let (_rest, (_index, name)) =
            inline_origin_line(input).map_err(|_| BreakpadParseError::ParsingFile)?;
        str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BreakpadIndexParser {
    line_buffer: LineBuffer,
    inner: BreakpadIndexParserInner,
}

impl BreakpadIndexParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn consume(&mut self, chunk: &[u8]) {
        let inner = &mut self.inner;
        let line_buffer = &mut self.line_buffer;
        line_buffer.consume(chunk, |offset, line| inner.process_line(offset, line));
    }

    pub fn finish(mut self) -> Result<BreakpadIndex, BreakpadParseError> {
        let inner = &mut self.inner;
        let final_offset = self
            .line_buffer
            .finish(|offset, line| inner.process_line(offset, line));
        self.inner.finish(final_offset)
    }
}

#[derive(Debug, Clone)]
struct SortedVecBuilder<I: FileOrInlineOrigin> {
    inner: Vec<I>,
    last_sorted_index: Option<u32>,
    is_sorted: bool,
}

impl<I: FileOrInlineOrigin> Default for SortedVecBuilder<I> {
    fn default() -> Self {
        Self {
            inner: Vec::new(),
            last_sorted_index: None,
            is_sorted: true,
        }
    }
}

impl<I: FileOrInlineOrigin> SortedVecBuilder<I> {
    pub fn push(&mut self, item: I) {
        if self.is_sorted {
            let item_index = item.index();
            match self.last_sorted_index {
                None => {
                    // This is the first item.
                    self.last_sorted_index = Some(item_index);
                }
                Some(last_index) if item_index > last_index => {
                    // This is the common case.
                    self.last_sorted_index = Some(item_index);
                }
                Some(last_index) if item_index == last_index => {
                    // Discard this item. We only keep the first item with this index.
                    // Valid files don't have duplicate indexes.
                    return;
                }
                Some(_last_index) => {
                    // item_index < last_index
                    self.is_sorted = false;
                }
            }
        }
        self.inner.push(item);
    }

    pub fn into_sorted_vec(mut self) -> Vec<I> {
        if !self.is_sorted {
            self.inner.sort_by_key(I::index);
            self.inner.dedup_by_key(|item| item.index());
        }
        self.inner
    }
}

#[derive(Debug, Clone, Default)]
struct BreakpadIndexParserInner {
    module_info_bytes: Vec<u8>,
    module_info: Option<(DebugId, String, String, String)>,
    name: Option<String>,
    code_id: Option<CodeId>,
    symbols: Vec<(u32, BreakpadSymbolType)>,
    files: SortedVecBuilder<BreakpadFileLine>,
    inline_origins: SortedVecBuilder<BreakpadInlineOriginLine>,
    pending_func_block: Option<(u32, u64)>,
}

impl BreakpadIndexParserInner {
    pub fn process_line(&mut self, file_offset: u64, line: &[u8]) {
        let mut input = line;
        while input.last() == Some(&b'\r') {
            input = &input[..(input.len() - 1)];
        }
        if self.module_info.is_none() {
            // Every file must start with a "MODULE " line.
            if let Ok((_r, (os, arch, debug_id, debug_name))) = module_line(input) {
                self.module_info = Some((
                    debug_id,
                    os.to_string(),
                    arch.to_string(),
                    debug_name.to_string(),
                ));
            }
            input.clone_into(&mut self.module_info_bytes);
            return;
        }
        let line_length = input.len() as u32;
        if let Ok((_r, (index, _filename))) = file_line(input) {
            self.files.push(BreakpadFileLine {
                index,
                file_offset,
                line_length,
            });
        } else if let Ok((_r, (index, _inline_origin))) = inline_origin_line(input) {
            self.inline_origins.push(BreakpadInlineOriginLine {
                index,
                file_offset,
                line_length,
            });
        } else if let Ok((_r, (address, _name))) = public_line(input) {
            self.finish_pending_func_block(file_offset);
            self.symbols.push((
                address,
                BreakpadSymbolType::Public(BreakpadPublicSymbol {
                    file_offset,
                    line_length,
                }),
            ));
        } else if let Ok((_r, (address, _size, _name))) = func_line(input) {
            self.finish_pending_func_block(file_offset);
            self.pending_func_block = Some((address, file_offset));
        } else if input.starts_with(b"INFO ") {
            self.finish_pending_func_block(file_offset);
            self.module_info_bytes.push(b'\n');
            self.module_info_bytes.extend_from_slice(input);
            if let Ok((_r, (code_id, name_str))) = info_code_id_line(input) {
                self.code_id = CodeId::from_str(code_id).ok();
                self.name = name_str.map(ToOwned::to_owned);
            }
        } else if input.starts_with(b"STACK ") {
            self.finish_pending_func_block(file_offset);
        }
    }

    fn finish_pending_func_block(&mut self, non_func_line_start_offset: u64) {
        if let Some((address, file_offset)) = self.pending_func_block.take() {
            let block_length = (non_func_line_start_offset - file_offset) as u32;
            self.symbols.push((
                address,
                BreakpadSymbolType::Func(BreakpadFuncSymbol {
                    file_offset,
                    block_length,
                }),
            ));
        }
    }

    pub fn finish(mut self, file_end_offset: u64) -> Result<BreakpadIndex, BreakpadParseError> {
        self.finish_pending_func_block(file_end_offset);
        let BreakpadIndexParserInner {
            mut symbols,
            module_info_bytes,
            files,
            inline_origins,
            module_info,
            name,
            code_id,
            ..
        } = self;
        symbols.sort_by_key(|(address, _)| *address);
        symbols.dedup_by_key(|(address, _)| *address);
        let (symbol_addresses, symbol_offsets) = symbols.into_iter().unzip();

        let files = ItemMap::from_sorted_vec(files.into_sorted_vec());
        let inline_origins = ItemMap::from_sorted_vec(inline_origins.into_sorted_vec());

        let (debug_id, os, arch, debug_name) =
            module_info.ok_or(BreakpadParseError::NoModuleInfoInSymFile)?;
        Ok(BreakpadIndex {
            module_info_bytes,
            debug_name,
            debug_id,
            code_id,
            name,
            arch,
            os,
            symbol_addresses,
            symbol_offsets,
            files,
            inline_origins,
        })
    }
}

/// Consumes chunks and calls a callback for each line.
/// Leftover pieces are stored in a dynamically growing `Vec` in this object.
#[derive(Debug, Clone, Default)]
pub struct LineBuffer {
    leftover_bytes: Vec<u8>,
    /// The current offset in the file, taking into account all the bytes
    /// that have been consumed from the chunks. This also counts bytes that
    /// have been "consumed" by having been transferred to `leftover_bytes`.
    current_offset: u64,
}

impl LineBuffer {
    pub fn consume(&mut self, mut chunk: &[u8], mut f: impl FnMut(u64, &[u8])) {
        assert!(
            self.leftover_bytes.len() as u64 <= self.current_offset,
            "Caller supplied more self.leftover_bytes than we could have read ourselves"
        );
        loop {
            match memchr(b'\n', chunk) {
                None => {
                    self.leftover_bytes.extend_from_slice(chunk);
                    self.current_offset += chunk.len() as u64;
                    return;
                }
                Some(line_break_pos_in_chunk) => {
                    let chunk_until_line_break = &chunk[..line_break_pos_in_chunk];
                    // let chunk_until_line_break = (&chunk[..line_break_pos_in_chunk]).trim_end_matches(b'\r');
                    chunk = &chunk[(line_break_pos_in_chunk + 1)..];
                    let (line, line_start_offset) = if self.leftover_bytes.is_empty() {
                        (chunk_until_line_break, self.current_offset)
                    } else {
                        let line_start_offset =
                            self.current_offset - (self.leftover_bytes.len() as u64);
                        self.leftover_bytes.extend(chunk_until_line_break);
                        (self.leftover_bytes.as_slice(), line_start_offset)
                    };
                    self.current_offset += line_break_pos_in_chunk as u64 + 1;
                    f(line_start_offset, line);
                    self.leftover_bytes.clear();
                }
            };
        }
    }

    pub fn finish(self, mut f: impl FnMut(u64, &[u8])) -> u64 {
        if !self.leftover_bytes.is_empty() {
            let line_start_offset = self.current_offset - (self.leftover_bytes.len() as u64);
            f(line_start_offset, &self.leftover_bytes);
        }
        self.current_offset
    }
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BreakpadParseError {
    #[error("Error parsing PUBLIC line")]
    ParsingPublic,

    #[error("Error parsing FILE line")]
    ParsingFile,

    #[error("Error parsing INLINE_ORIGIN line")]
    ParsingInlineOrigin,

    #[error("Error parsing FUNC line")]
    ParsingFunc,

    #[error("Error parsing INLINE line")]
    ParsingInline,

    #[error("Error parsing func line data line")]
    ParsingFuncLine,

    #[error("Malformed UTF-8")]
    BadUtf8,

    #[error("The Breakpad sym file did not start with a valid MODULE line")]
    NoModuleInfoInSymFile,
}

#[derive(Debug, Clone)]
pub struct BreakpadPublicSymbolInfo<'a> {
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct BreakpadFuncSymbolInfo<'a> {
    pub name: &'a str,
    pub size: u32,
    pub lines: Vec<SourceLine>,
    pub inlinees: Vec<Inlinee>,
}

impl<'a> BreakpadFuncSymbolInfo<'a> {
    /// Returns `(file_id, line, address)` of the line record that covers the
    /// given address. Line records describe locations at the deepest level of
    /// inlining at that address.
    ///
    /// For example, if we have an "inline call stack" A -> B -> C at this
    /// address, i.e. both the call to B and the call to C have been inlined all
    /// the way into A (A being the "outer function"), then this method reports
    /// locations in C.
    pub fn get_innermost_sourceloc(&self, addr: u32) -> Option<&SourceLine> {
        let line_index = match self.lines.binary_search_by_key(&addr, |line| line.address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        Some(&self.lines[line_index])
    }

    /// Returns `(call_file_id, call_line, address, inline_origin)` of the
    /// inlinee record that covers the given address at the given depth.
    ///
    /// We start at depth zero. For example, if we have an "inline call stack"
    /// A -> B -> C at an address, i.e. both the call to B and the call to C have
    /// been inlined all the way into A (A being the "outer function"), then the
    /// call A -> B is at level zero, and the call B -> C is at level one.
    pub fn get_inlinee_at_depth(&self, depth: u32, addr: u32) -> Option<&Inlinee> {
        let index = match self
            .inlinees
            .binary_search_by_key(&(depth, addr), |inlinee| (inlinee.depth, inlinee.address))
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let inlinee = &self.inlinees[index];
        if inlinee.depth != depth {
            return None;
        }
        let end_address = inlinee.address.checked_add(inlinee.size)?;
        if addr < end_address {
            Some(inlinee)
        } else {
            None
        }
    }
}

/// Match a hex string, parse it to a u32 or a u64.
fn hex_str<T: std::ops::Shl<T, Output = T> + std::ops::BitOr<T, Output = T> + From<u8>>(
    input: &[u8],
) -> IResult<&[u8], T> {
    // Consume up to max_len digits. For u32 that's 8 digits and for u64 that's 16 digits.
    // Two hex digits form one byte.
    let max_len = mem::size_of::<T>() * 2;

    let mut res: T = T::from(0);
    let mut k = 0;
    for v in input.iter().take(max_len) {
        let digit = match (*v as char).to_digit(16) {
            Some(v) => v,
            None => break,
        };
        res = res << T::from(4);
        res = res | T::from(digit as u8);
        k += 1;
    }
    if k == 0 {
        return Err(Err::Error(Error::from_error_kind(
            input,
            ErrorKind::HexDigit,
        )));
    }
    let remaining = &input[k..];
    Ok((remaining, res))
}

/// Match a decimal string, parse it to a u32.
///
/// This is doing everything manually so that we only look at each byte once.
/// With a naive implementation you might be looking at them three times: First
/// you might get a slice of acceptable characters from nom, then you might parse
/// that slice into a str (checking for utf-8 unnecessarily), and then you might
/// parse that string into a decimal number.
fn decimal_u32(input: &[u8]) -> IResult<&[u8], u32> {
    const MAX_LEN: usize = 10; // u32::MAX has 10 decimal digits
    let mut res: u64 = 0;
    let mut k = 0;
    for v in input.iter().take(MAX_LEN) {
        let digit = *v as char;
        let digit_value = match digit.to_digit(10) {
            Some(v) => v,
            None => break,
        };
        res = res * 10 + digit_value as u64;
        k += 1;
    }
    if k == 0 {
        return Err(Err::Error(Error::from_error_kind(input, ErrorKind::Digit)));
    }
    let res = u32::try_from(res)
        .map_err(|_| Err::Error(Error::from_error_kind(input, ErrorKind::TooLarge)))?;
    let remaining = &input[k..];
    Ok((remaining, res))
}

/// Take 0 or more non-space bytes.
fn non_space(input: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while(|c: u8| c != b' ')(input)
}

// Matches a MODULE record.
fn module_line(input: &[u8]) -> IResult<&[u8], (&str, &str, DebugId, &str)> {
    let (input, _) = terminated(tag("MODULE"), space1)(input)?;
    let (input, (os, cpu, debug_id, name)) = cut(tuple((
        terminated(map_res(non_space, str::from_utf8), space1), // os
        terminated(map_res(non_space, str::from_utf8), space1), // cpu
        terminated(
            map_res(map_res(hex_digit1, str::from_utf8), DebugId::from_breakpad),
            space1,
        ), // debug id
        map_res(rest, str::from_utf8),                          // filename
    )))(input)?;
    Ok((input, (os, cpu, debug_id, name)))
}

// Matches an INFO CODE_ID record.
fn info_code_id_line(input: &[u8]) -> IResult<&[u8], (&str, Option<&str>)> {
    let (input, _) = terminated(tag("INFO CODE_ID"), space1)(input)?;
    let (input, code_id_with_name) = map_res(rest, str::from_utf8)(input)?;
    match code_id_with_name.split_once(' ') {
        Some((code_id, name)) => Ok((input, (code_id, Some(name)))),
        None => Ok((input, (code_id_with_name, None))),
    }
}

// Matches a FILE record.
fn file_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("FILE"), space1)(input)?;
    let (input, (id, filename)) = cut(tuple((terminated(decimal_u32, space1), rest)))(input)?;
    Ok((input, (id, filename)))
}

// Matches an INLINE_ORIGIN record.
fn inline_origin_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("INLINE_ORIGIN"), space1)(input)?;
    let (input, (id, function)) = cut(tuple((terminated(decimal_u32, space1), rest)))(input)?;
    Ok((input, (id, function)))
}

// Matches a PUBLIC record.
fn public_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("PUBLIC"), space1)(input)?;
    let (input, (_multiple, address, _parameter_size, name)) = cut(tuple((
        opt(terminated(tag("m"), space1)),
        terminated(hex_str::<u64>, space1),
        terminated(hex_str::<u32>, space1),
        rest,
    )))(input)?;
    Ok((input, (address as u32, name)))
}

/// A mapping from machine code bytes to source line and file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    /// The start address relative to the module's load address.
    pub address: u32,
    /// The size of this range of instructions in bytes.
    pub size: u32,
    /// The source file name that generated this machine code.
    ///
    /// This is an index into `SymbolFile::files`.
    pub file: u32,
    /// The line number in `file` that generated this machine code.
    pub line: u32,
}

/// A single range which is covered by an inlined function call.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Inlinee {
    /// The depth of the inline call.
    pub depth: u32,
    /// The start address relative to the module's load address.
    pub address: u32,
    /// The size of this range of instructions in bytes.
    pub size: u32,
    /// The source file which contains the function call.
    ///
    /// This is an index into `SymbolFile::files`.
    pub call_file: u32,
    /// The line number in `call_file` for the function call.
    pub call_line: u32,
    /// The function name, as an index into `SymbolFile::inline_origins`.
    pub origin_id: u32,
}

// Matches line data after a FUNC record.
///
/// A line record has the form <hex_addr> <hex_size> <line> <file_id>
fn func_line_data(input: &[u8]) -> IResult<&[u8], SourceLine> {
    let (input, (address, size, line, file)) = tuple((
        terminated(hex_str::<u64>, space1),
        terminated(hex_str::<u32>, space1),
        terminated(decimal_u32, space1),
        decimal_u32,
    ))(input)?;
    Ok((
        input,
        SourceLine {
            address: address as u32,
            size,
            file,
            line,
        },
    ))
}

// Matches a FUNC record.
fn func_line(input: &[u8]) -> IResult<&[u8], (u32, u32, &[u8])> {
    let (input, _) = terminated(tag("FUNC"), space1)(input)?;
    let (input, (_multiple, address, size, _parameter_size, name)) = cut(tuple((
        opt(terminated(tag("m"), space1)),
        terminated(hex_str::<u32>, space1),
        terminated(hex_str::<u32>, space1),
        terminated(hex_str::<u32>, space1),
        rest,
    )))(input)?;
    Ok((input, (address, size, name)))
}

// Matches one entry of the form <address> <size> which is used at the end of an INLINE record
fn inline_address_range(input: &[u8]) -> IResult<&[u8], (u32, u32)> {
    tuple((terminated(hex_str::<u32>, space1), hex_str::<u32>))(input)
}

// Matches an INLINE record.
///
/// An INLINE record has the form `INLINE <inline_nest_level> <call_site_line> <call_site_file_id> <origin_id> [<address> <size>]+`.
fn inline_line(input: &[u8]) -> IResult<&[u8], impl Iterator<Item = Inlinee>> {
    let (input, _) = terminated(tag("INLINE"), space1)(input)?;
    let (input, (depth, call_line, call_file, origin_id)) = cut(tuple((
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
    )))(input)?;
    let (input, address_ranges) = cut(separated_list1(space1, inline_address_range))(input)?;
    Ok((
        input,
        address_ranges
            .into_iter()
            .map(move |(address, size)| Inlinee {
                address,
                size,
                call_file,
                call_line,
                depth,
                origin_id,
            }),
    ))
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use super::*;
    use crate::{ElfBuildId, PeCodeId};

    #[test]
    fn test1() {
        let mut parser = BreakpadIndexParser::new();
        parser.consume(b"MODULE Linux x86_64 39CA3106713C8D0FFEE4605AFA2526670 libmozsandbox.so\nINFO CODE_ID ");
        parser.consume(b"0631CA393C710F8DFEE4605AFA2526671AD4EF17\nFILE 0 hg:hg.mozilla.org/mozilla-central:se");
        parser.consume(b"curity/sandbox/chromium/base/strings/safe_sprintf.cc:f150bc1f71d09e1e1941065951f0f5a3");
        parser.consume(b"8628f080");
        let index = parser.finish().unwrap();
        assert_eq!(
            index.files.get(0).unwrap(),
            &BreakpadFileLine {
                index: 0,
                file_offset: 125,
                line_length: 136,
            }
        );
        assert_eq!(
            index.debug_id,
            DebugId::from_breakpad("39CA3106713C8D0FFEE4605AFA2526670").unwrap()
        );
        assert_eq!(
            index.code_id,
            Some(CodeId::ElfBuildId(
                ElfBuildId::from_str("0631ca393c710f8dfee4605afa2526671ad4ef17").unwrap()
            ))
        );

        let v = index.serialize_to_bytes();
        let index2 = BreakpadIndex::parse_symindex_file(&v).unwrap();

        assert_eq!(index2, index);
    }

    #[test]
    fn test2() {
        let mut parser = BreakpadIndexParser::new();
        parser.consume(b"MODULE windows x86_64 F1E853FD662672044C4C44205044422E1 firefox.pdb\nIN");
        parser.consume(b"FO CODE_ID 63C036DBA7000 firefox.exe\nINFO GENERATOR mozilla/dump_syms ");
        parser.consume(b"2.1.1\nFILE 0 /builds/worker/workspace/obj-build/browser/app/d:/agent/_");
        parser.consume(b"work/2/s/src/vctools/delayimp/dloadsup.h\nFILE 1 /builds/worker/workspa");
        parser.consume(b"ce/obj-build/browser/app/d:/agent/_work/2/s/src/externalapis/windows/10");
        parser.consume(b"/sdk/inc/winnt.h\nINLINE_ORIGIN 0 DloadLock()\nINLINE_ORIGIN 1 DloadUnl");
        parser.consume(b"ock()\nINLINE_ORIGIN 2 WritePointerRelease(void**, void*)\nINLINE_ORIGI");
        parser.consume(b"N 3 WriteRelease64(long long*, long long)\nFUNC 2b754 aa 0 DloadAcquire");
        parser.consume(b"SectionWriteAccess()\nINLINE 0 658 0 0 2b76a 3d\nINLINE 0 665 0 1 2b7ca");
        parser.consume(b" 17 2b7e6 12\nINLINE 1 345 0 2 2b7ed b\nINLINE 2 8358 1 3 2b7ed b\n2b75");
        parser.consume(b"4 6 644 0\n2b75a 10 650 0\n2b76a e 299 0\n2b778 14 300 0\n2b78c 2 301 0");
        parser.consume(b"\n2b78e 2 306 0\n2b790 c 305 0\n2b79c b 309 0\n2b7a7 10 660 0\n2b7b7 2 ");
        parser.consume(b"661 0\n2b7b9 11 662 0\n2b7ca 9 340 0\n2b7d3 e 341 0\n2b7e1 c 668 0\n2b7");
        parser.consume(b"ed b 7729 1\n2b7f8 6 668 0");
        let index = parser.finish().unwrap();
        assert_eq!(&index.debug_name, "firefox.pdb");
        assert_eq!(
            index.debug_id,
            DebugId::from_breakpad("F1E853FD662672044C4C44205044422E1").unwrap()
        );
        assert_eq!(index.name.as_deref(), Some("firefox.exe"));
        assert_eq!(
            index.code_id,
            Some(CodeId::PeCodeId(
                PeCodeId::from_str("63C036DBA7000").unwrap()
            ))
        );
        assert!(std::str::from_utf8(&index.module_info_bytes)
            .unwrap()
            .contains("INFO GENERATOR mozilla/dump_syms 2.1.1"));

        let v = index.serialize_to_bytes();
        let index2 = BreakpadIndex::parse_symindex_file(&v).unwrap();

        assert_eq!(index2, index);
    }

    #[test]
    fn func_parsing() {
        let block =
            b"JUNK\nFUNC 1130 28 0 main\n1130 f 24 0\n113f 7 25 0\n1146 9 26 0\n114f 9 27 0\nJUNK";
        let func = BreakpadFuncSymbol {
            file_offset: "JUNK\n".len() as u64,
            block_length: (block.len() - "JUNK\n".len() - "\nJUNK".len()) as u32,
        };
        let input = &block[func.file_offset as usize..][..func.block_length as usize];
        let func = func.parse(input).unwrap();
        assert_eq!(func.name, "main");
        assert_eq!(func.size, 0x28);
        assert_eq!(func.lines.len(), 4);
        assert_eq!(
            func.lines[0],
            SourceLine {
                address: 0x1130,
                size: 0xf,
                file: 0,
                line: 24,
            }
        );
        assert_eq!(
            func.lines[3],
            SourceLine {
                address: 0x114f,
                size: 0x9,
                file: 0,
                line: 27,
            }
        );
    }
}
