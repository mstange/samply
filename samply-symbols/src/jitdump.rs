use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use debugid::DebugId;
use linux_perf_data::jitdump::{
    JitCodeDebugInfoRecord, JitCodeLoadRecord, JitDumpReader, JitDumpRecord, JitDumpRecordHeader,
    JitDumpRecordType,
};
use linux_perf_data::linux_perf_event_reader::RawData;
use linux_perf_data::Endianness;
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::error::Error;
use crate::generation::SymbolMapGeneration;
use crate::shared::{
    FileContents, FileContentsCursor, FileContentsWrapper, FrameDebugInfo, FramesLookupResult,
    LookupAddress, SymbolInfo,
};
use crate::symbol_map::{GetInnerSymbolMap, SymbolMap, SymbolMapTrait};
use crate::symbol_map_string_interner::SymbolMapStringHandle;
use crate::{FileAndPathHelper, SourceFilePath, SourceFilePathHandle, SyncAddressInfo};
use crate::{FunctionNameHandle, SymbolMapStringInterner, SymbolNameHandle};

pub fn is_jitdump_file<T: FileContents>(file_contents: &FileContentsWrapper<T>) -> bool {
    const MAGIC_BYTES_BE: &[u8] = b"JiTD";
    const MAGIC_BYTES_LE: &[u8] = b"DTiJ";
    matches!(
        file_contents.read_bytes_at(0, 4),
        Ok(MAGIC_BYTES_BE | MAGIC_BYTES_LE)
    )
}

/// This makes up an ID which looks like an ELF build ID, composed of
/// information from the jitdump file header.
pub fn debug_id_and_code_id_for_jitdump(
    pid: u32,
    timestamp: u64,
    elf_machine_arch: u32,
) -> (DebugId, [u8; 20]) {
    let mut code_id_bytes = [0; 20];
    code_id_bytes[0..4].copy_from_slice(b"JITD");
    code_id_bytes[4..8].copy_from_slice(&pid.to_le_bytes());
    code_id_bytes[8..16].copy_from_slice(&timestamp.to_le_bytes());
    code_id_bytes[16..20].copy_from_slice(&elf_machine_arch.to_le_bytes());
    let debug_id = DebugId::from_guid_age(&code_id_bytes[..16], 0).unwrap();
    (debug_id, code_id_bytes)
}

const JS_PREFIXES_WITH_SPACE_SEPARATED_FILENAME_AT_THE_END: &[&str] = &[
    // JSC categories.
    "JSC-Baseline: ",
    "JSC-DFG: ",
    "JSC-FTL: ",
    "JSC-WasmOMG: ",
    "JSC-WasmBBQ: ",
    // V8 prefixes: https://source.chromium.org/chromium/chromium/src/+/main:v8/src/objects/code-kind.cc;l=21;drc=52e0bed2a5bce0ccc0894c03bdd7a6e13e6aff50
    "JS:~",
    "Script:~",
    "JS:^",
    "JS:+'",
    "JS:+",
    "JS:o+",
    "JS:*'",
    "JS:*",
    "JS:o*",
    "JS:?",
];

const JS_PREFIXES_WITH_PARENTHESIZED_FILENAME_AT_THE_END: &[&str] =
    &["Interpreter: ", "Baseline: ", "Ion: ", "Wasm: "];

const JS_PREFIXES_WITHOUT_FILENAME: &[&str] = &["py::"];

struct JsFileInfo<'a> {
    filename: &'a str,
    start_line: Option<u32>,
    start_col: Option<u32>,
}

impl<'a> JsFileInfo<'a> {
    pub fn file_only(filename: &'a str) -> Self {
        Self {
            filename,
            start_line: None,
            start_col: None,
        }
    }
    pub fn file_and_line(filename: &'a str, line: u32) -> Self {
        Self {
            filename,
            start_line: Some(line),
            start_col: None,
        }
    }
    pub fn file_and_line_and_col(filename: &'a str, line: u32, col: u32) -> Self {
        Self {
            filename,
            start_line: Some(line),
            start_col: Some(col),
        }
    }
}

fn split_colon_separated_number_from_end(s: &str) -> Option<(&str, u32)> {
    let (rest, suffix) = s.rsplit_once(':')?;
    let num = suffix.parse::<u32>().ok()?;
    Some((rest, num))
}

/// Split "filename.js:4:5" into ("filename.js", 4, 5)
fn extract_file_info(file: &str) -> JsFileInfo<'_> {
    let Some((rest, last_num)) = split_colon_separated_number_from_end(file) else {
        return JsFileInfo::file_only(file);
    };
    match split_colon_separated_number_from_end(rest) {
        Some((rest, second_last_num)) => {
            JsFileInfo::file_and_line_and_col(rest, second_last_num, last_num)
        }
        None => JsFileInfo::file_and_line(rest, last_num),
    }
}

fn parse_js_jit_function_name(raw_name: &str) -> Option<(&str, Option<&str>)> {
    for prefix in JS_PREFIXES_WITH_SPACE_SEPARATED_FILENAME_AT_THE_END {
        let Some(rest) = raw_name.strip_prefix(prefix) else {
            continue;
        };
        if let Some((name, filename)) = rest.rsplit_once(' ') {
            return Some((name, Some(filename)));
        }
        return Some((rest, None));
    }
    for prefix in JS_PREFIXES_WITH_PARENTHESIZED_FILENAME_AT_THE_END {
        let Some(rest) = raw_name.strip_prefix(prefix) else {
            continue;
        };
        if let Some(without_trailing_paren) = rest.strip_suffix(')') {
            if let Some((name, filename)) = without_trailing_paren.rsplit_once(" (") {
                return Some((name, Some(filename)));
            }
        }
        return Some((rest, None));
    }
    for prefix in JS_PREFIXES_WITHOUT_FILENAME {
        let Some(rest) = raw_name.strip_prefix(prefix) else {
            continue;
        };
        return Some((rest, None));
    }
    None
}

const SAMPLY_JIT_CODE_DEBUG_INFO2: JitDumpRecordType = JitDumpRecordType(827346260);

#[derive(Debug)]
struct SamplyJitCodeDebugInfo2Record<'a> {
    #[allow(unused)]
    code_addr: u64,

    code_offsets: RawDataElemSlice<'a, CodeOffsetRecord>,
    source_positions: RawDataElemSlice<'a, SourcePosition>,

    strings: Vec<RawData<'a>>,
}

struct CodeOffsetRecord {
    /// The code address offset, relative to the function start address.
    /// This record describes all instructions starting at this offset,
    /// up to the next code offset record's offset.
    code_offset: u32,
    /// The index of the source position.
    source_position: u32,
}

#[derive(Debug, Clone, Copy)]
struct SourcePosition {
    /// The index of the caller source position, if this source position
    /// is within an inlined function. Otherwise -1.
    caller_index: i32,
    /// The name of the function. This is an index into the string list.
    function_name: u32,
    /// The name of the file which the function is declared in. This is
    /// an index into the string list. -1 if unknown.
    file: i32,
    /// The line number where the function starts, 1-based. 0 if unknown.
    function_start_line: u32,
    /// The column number where the function starts, 1-based. 0 if unknown.
    function_start_column: u32,
    /// The line number, 1-based. 0 if unknown.
    line: u32,
    /// The column number, 1-based. 0 if unknown.
    column: u32,
}

trait InfallibleParseFromRawData<'a> {
    const DATA_LEN: usize;

    /// Receives a RawData of len DATA_LEN
    fn parse(endian: Endianness, data: RawData<'a>) -> Self;
}

struct RawDataElemSlice<'a, T> {
    endian: Endianness,
    data: RawData<'a>,
    len: usize,
    _phantom: PhantomData<&'a T>,
}

impl<'a, T: InfallibleParseFromRawData<'a>> RawDataElemSlice<'a, T> {
    pub fn new(endian: Endianness, data: RawData<'a>, len: usize) -> Self {
        assert_eq!(data.len(), len * T::DATA_LEN);
        Self {
            endian,
            data,
            len,
            _phantom: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }

        let data = self.data.get(index * T::DATA_LEN..self.data.len()).unwrap();
        Some(T::parse(self.endian, data))
    }

    #[inline]
    pub fn binary_search_by<F>(&self, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&T) -> Ordering,
    {
        let mut size = self.len();
        if size == 0 {
            return Err(0);
        }
        let mut base = 0usize;

        while size > 1 {
            let half = size / 2;
            let mid = base + half;

            let cmp = f(&self.get(mid).unwrap());
            base = if cmp == Ordering::Greater { base } else { mid };
            size -= half;
        }

        let cmp = f(&self.get(base).unwrap());
        if cmp == Ordering::Equal {
            Ok(base)
        } else {
            let result = base + (cmp == Ordering::Less) as usize;
            Err(result)
        }
    }

    #[inline]
    pub fn binary_search_by_key<B, F>(&self, b: &B, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&T) -> B,
        B: Ord,
    {
        self.binary_search_by(|k| f(k).cmp(b))
    }
}

impl<'a, T: InfallibleParseFromRawData<'a> + std::fmt::Debug> std::fmt::Debug
    for RawDataElemSlice<'a, T>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries((0..self.len).map(|i| self.get(i).unwrap()))
            .finish()
    }
}

impl std::fmt::Debug for CodeOffsetRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeOffsetRecord")
            .field("code_offset", &format_args!("{:#x}", self.code_offset))
            .field("source_position", &self.source_position)
            .finish()
    }
}

impl<'a> SamplyJitCodeDebugInfo2Record<'a> {
    pub fn parse(endian: Endianness, data: RawData<'a>) -> Result<Self, std::io::Error> {
        let (code_addr, code_offset_count, source_position_count, string_count, remaining_data) =
            match endian {
                Endianness::LittleEndian => Self::parse_fields::<LittleEndian>(data)?,
                Endianness::BigEndian => Self::parse_fields::<BigEndian>(data)?,
            };

        let mut cur = remaining_data;
        let code_offset_list_bytes =
            cur.split_off_prefix(size_of::<CodeOffsetRecord>() * code_offset_count)?;
        let source_position_list_bytes =
            cur.split_off_prefix(size_of::<SourcePosition>() * source_position_count)?;

        let mut strings = Vec::new();

        for i in 0..string_count {
            let Some(s) = cur.read_string() else {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Couldn't find NUL byte for string at index {i} in SAMPLY_JIT_CODE_DEBUG_INFO2 record for address {code_addr:#x}")));
            };
            strings.push(s);
        }

        Ok(Self {
            code_addr,
            code_offsets: RawDataElemSlice::new(endian, code_offset_list_bytes, code_offset_count),
            source_positions: RawDataElemSlice::new(
                endian,
                source_position_list_bytes,
                source_position_count,
            ),
            strings,
        })
    }

    fn parse_fields<O: ByteOrder>(
        data: RawData<'a>,
    ) -> Result<(u64, usize, usize, u64, RawData<'a>), std::io::Error> {
        let mut cur = data;
        let code_addr = cur.read_u64::<O>()?;
        let code_offset_count = usize::try_from(cur.read_u64::<O>()?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let source_position_count = usize::try_from(cur.read_u64::<O>()?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let string_count = cur.read_u64::<O>()?;
        Ok((
            code_addr,
            code_offset_count,
            source_position_count,
            string_count,
            cur,
        ))
    }

    pub fn get_string(&self, index: usize) -> Option<RawData<'a>> {
        self.strings.get(index).copied()
    }
}

impl CodeOffsetRecord {
    pub fn parse(endian: Endianness, data: RawData<'_>) -> Result<Self, std::io::Error> {
        match endian {
            Endianness::LittleEndian => Self::parse_impl::<LittleEndian>(data),
            Endianness::BigEndian => Self::parse_impl::<BigEndian>(data),
        }
    }

    pub fn parse_impl<O: ByteOrder>(data: RawData<'_>) -> Result<Self, std::io::Error> {
        let mut cur = data;
        let code_offset = cur.read_u32::<O>()?;
        let source_position = cur.read_u32::<O>()?;
        Ok(Self {
            code_offset,
            source_position,
        })
    }
}

impl<'a> InfallibleParseFromRawData<'a> for CodeOffsetRecord {
    const DATA_LEN: usize = size_of::<CodeOffsetRecord>();
    fn parse(endian: Endianness, data: RawData<'a>) -> Self {
        Self::parse(endian, data).unwrap()
    }
}

impl SourcePosition {
    pub fn parse(endian: Endianness, data: RawData<'_>) -> Result<Self, std::io::Error> {
        match endian {
            Endianness::LittleEndian => Self::parse_impl::<LittleEndian>(data),
            Endianness::BigEndian => Self::parse_impl::<BigEndian>(data),
        }
    }

    pub fn parse_impl<O: ByteOrder>(data: RawData<'_>) -> Result<Self, std::io::Error> {
        let mut cur = data;
        let caller_index = cur.read_i32::<O>()?;
        let function_name = cur.read_u32::<O>()?;
        let file = cur.read_i32::<O>()?;
        let function_start_line = cur.read_u32::<O>()?;
        let function_start_column = cur.read_u32::<O>()?;
        let line = cur.read_u32::<O>()?;
        let column = cur.read_u32::<O>()?;
        Ok(Self {
            caller_index,
            function_name,
            file,
            line,
            column,
            function_start_line,
            function_start_column,
        })
    }
}

impl<'a> InfallibleParseFromRawData<'a> for SourcePosition {
    const DATA_LEN: usize = size_of::<SourcePosition>();
    fn parse(endian: Endianness, data: RawData<'a>) -> Self {
        Self::parse(endian, data).unwrap()
    }
}

#[derive(Debug, Clone)]
pub struct JitDumpIndex {
    pub endian: Endianness,
    pub entries: Vec<JitDumpIndexEntry>,
    pub relative_addresses: Vec<u32>,
    pub debug_id: DebugId,
}

impl JitDumpIndex {
    pub fn from_reader<R: std::io::Read + std::io::Seek>(
        mut reader: JitDumpReader<R>,
    ) -> Result<Self, std::io::Error> {
        let header = reader.header();
        let (debug_id, _code_id_bytes) =
            debug_id_and_code_id_for_jitdump(header.pid, header.timestamp, header.elf_machine_arch);

        let endian = reader.endian();

        let mut entries = Vec::new();
        let mut relative_addresses = Vec::new();
        let mut cumulative_address = 0;
        let mut offset_and_len_of_pending_debug_info_record = None;
        let mut offset_and_len_of_pending_debug_info2_record = None;
        while let Some(record_header) = reader.next_record_header()? {
            match record_header.record_type {
                JitDumpRecordType::JIT_CODE_LOAD => {
                    // Read the full record.
                    let Some(raw_record) = reader.next_record()? else {
                        break;
                    };
                    let JitDumpRecord::CodeLoad(record) = raw_record.parse()? else {
                        panic!()
                    };
                    let code_debug_info_record_offset_and_len =
                        offset_and_len_of_pending_debug_info_record.take();
                    let code_debug_info2_record_offset_and_len =
                        offset_and_len_of_pending_debug_info2_record.take();
                    let relative_address = cumulative_address;
                    cumulative_address += record.code_bytes.len() as u32;
                    // eprintln!("JIT_CODE_LOAD at {} for function {}", raw_record.start_offset, std::str::from_utf8(&record.function_name.as_slice()).unwrap());

                    entries.push(JitDumpIndexEntry {
                        code_load_record_offset: raw_record.start_offset,
                        code_bytes_offset: raw_record.start_offset
                            + record.code_bytes_offset_from_record_header_start() as u64,
                        name_len: record.function_name.len() as u32,
                        code_debug_info_record_offset_and_len,
                        code_debug_info2_record_offset_and_len,
                        code_bytes_len: record.code_bytes.len() as u64,
                    });
                    relative_addresses.push(relative_address);
                }
                JitDumpRecordType::JIT_CODE_DEBUG_INFO => {
                    let offset = reader.next_record_offset();
                    if !reader.skip_next_record()? {
                        break;
                    }
                    offset_and_len_of_pending_debug_info_record =
                        Some((offset, record_header.total_size));
                }
                SAMPLY_JIT_CODE_DEBUG_INFO2 => {
                    let offset = reader.next_record_offset();
                    if !reader.skip_next_record()? {
                        break;
                    }
                    offset_and_len_of_pending_debug_info2_record =
                        Some((offset, record_header.total_size));
                }
                _ => {
                    // Skip other record types.
                    if !reader.skip_next_record()? {
                        break;
                    }
                }
            }
        }
        Ok(Self {
            endian,
            entries,
            relative_addresses,
            debug_id,
        })
    }

    /// Returns (entry index, entry relative address, offset from entry start)
    pub fn lookup_relative_address(&self, address: u32) -> Option<(usize, u32, u32)> {
        let index = match self.relative_addresses.binary_search(&address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_address = self.relative_addresses[index];
        let offset_relative_to_symbol = address - symbol_address;
        let desc = &self.entries[index];
        if u64::from(offset_relative_to_symbol) >= desc.code_bytes_len {
            return None;
        }
        Some((index, symbol_address, offset_relative_to_symbol))
    }

    /// Returns (entry index, entry relative address, offset from entry start)
    pub fn lookup_offset(&self, offset: u64) -> Option<(usize, u32, u32)> {
        let index = match self
            .entries
            .binary_search_by_key(&offset, |entry| entry.code_bytes_offset)
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_code_bytes_offset = self.entries[index].code_bytes_offset;
        let offset_relative_to_symbol = u32::try_from(offset - symbol_code_bytes_offset).ok()?;
        let desc = &self.entries[index];
        if u64::from(offset_relative_to_symbol) >= desc.code_bytes_len {
            return None;
        }
        let symbol_address = self.relative_addresses[index];
        Some((index, symbol_address, offset_relative_to_symbol))
    }
}

#[derive(Debug, Clone)]
pub struct JitDumpIndexEntry {
    pub code_load_record_offset: u64,
    pub code_bytes_offset: u64,
    pub name_len: u32,
    pub code_debug_info_record_offset_and_len: Option<(u64, u32)>,
    pub code_debug_info2_record_offset_and_len: Option<(u64, u32)>,
    pub code_bytes_len: u64,
}

pub fn get_symbol_map_for_jitdump<H: FileAndPathHelper>(
    file_contents: FileContentsWrapper<H::F>,
    file_location: H::FL,
) -> Result<SymbolMap<H>, Error> {
    let outer = JitDumpSymbolMapOuter::new(file_contents)?;
    let symbol_map = JitDumpSymbolMap(Yoke::attach_to_cart(Box::new(outer), |outer| {
        outer.make_symbol_map()
    }));
    Ok(SymbolMap::new_plain(file_location, Box::new(symbol_map)))
}

pub struct JitDumpSymbolMap<T: FileContents>(
    Yoke<JitDumpSymbolMapInnerWrapper<'static>, Box<JitDumpSymbolMapOuter<T>>>,
);

impl<T: FileContents> GetInnerSymbolMap for JitDumpSymbolMap<T> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a) {
        self.0.get().0.as_ref()
    }
}

pub struct JitDumpSymbolMapOuter<T: FileContents> {
    data: FileContentsWrapper<T>,
    index: JitDumpIndex,
}

impl<T: FileContents> JitDumpSymbolMapOuter<T> {
    pub fn new(data: FileContentsWrapper<T>) -> Result<Self, Error> {
        let cursor = FileContentsCursor::new(&data);
        let reader = JitDumpReader::new(cursor)?;
        let index = JitDumpIndex::from_reader(reader).map_err(Error::JitDumpFileReading)?;
        Ok(Self { data, index })
    }

    pub fn make_symbol_map(&self) -> JitDumpSymbolMapInnerWrapper<'_> {
        let inner = JitDumpSymbolMapInner {
            index: &self.index,
            cache: Mutex::new(JitDumpSymbolMapCache::new(
                &self.data,
                &self.index,
                SymbolMapGeneration::new(),
            )),
        };
        JitDumpSymbolMapInnerWrapper(Box::new(inner))
    }
}

struct JitDumpSymbolMapInner<'a, T: FileContents> {
    index: &'a JitDumpIndex,
    cache: Mutex<JitDumpSymbolMapCache<'a, T>>,
}

#[derive(Yokeable)]
pub struct JitDumpSymbolMapInnerWrapper<'data>(pub Box<dyn SymbolMapTrait + Send + Sync + 'data>);

#[derive(Debug)]
struct JitDumpSymbolMapCache<'a, T: FileContents> {
    pub inner: JitDumpSymbolMapCacheInner<'a, T>,
    pub string_interner: RawDataSymbolMapStringInterner<'a>,
}

#[derive(Debug)]
struct JitDumpSymbolMapCacheInner<'a, T: FileContents> {
    names: HashMap<usize, &'a [u8]>,
    debug_infos: HashMap<usize, JitDumpDebugInfoEnum<'a>>,
    data: &'a FileContentsWrapper<T>,
    index: &'a JitDumpIndex,
}

#[derive(Debug)]
enum JitDumpDebugInfoEnum<'a> {
    V1(JitCodeDebugInfoRecord<'a>),
    V2(SamplyJitCodeDebugInfo2Record<'a>),
}

#[derive(Debug)]
struct RawDataSymbolMapStringInterner<'a>(SymbolMapStringInterner<'a>);

impl<'a, T: FileContents> JitDumpSymbolMapCache<'a, T> {
    pub fn new(
        data: &'a FileContentsWrapper<T>,
        index: &'a JitDumpIndex,
        generation: SymbolMapGeneration,
    ) -> Self {
        Self {
            inner: JitDumpSymbolMapCacheInner::new(data, index),
            string_interner: RawDataSymbolMapStringInterner(SymbolMapStringInterner::new(
                generation,
            )),
        }
    }
}

impl<'a, T: FileContents> JitDumpSymbolMapCacheInner<'a, T> {
    pub fn new(data: &'a FileContentsWrapper<T>, index: &'a JitDumpIndex) -> Self {
        Self {
            names: HashMap::new(),
            debug_infos: HashMap::new(),
            data,
            index,
        }
    }

    pub fn get_function_name(&mut self, entry_index: usize) -> Option<&'a [u8]> {
        let name = match self.names.entry(entry_index) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let desc = &self.index.entries[entry_index];
                let load_record_start = desc.code_load_record_offset;
                let name_start =
                    load_record_start + JitCodeLoadRecord::NAME_OFFSET_FROM_RECORD_START as u64;
                let name_len = desc.name_len as u64;
                let name = self.data.read_bytes_at(name_start, name_len).ok()?;
                entry.insert(name)
            }
        };
        Some(name)
    }

    pub fn get_debug_info(&mut self, entry_index: usize) -> Option<&JitDumpDebugInfoEnum<'a>> {
        let entry = match self.debug_infos.entry(entry_index) {
            Entry::Occupied(entry) => return Some(entry.into_mut()),
            Entry::Vacant(entry) => entry,
        };
        let desc = &self.index.entries[entry_index];
        let record = if let Some((record_start, record_len)) =
            desc.code_debug_info2_record_offset_and_len
        {
            let record_data = self
                .data
                .read_bytes_at(record_start, record_len.into())
                .ok()?;
            let mut record_data = RawData::Single(record_data);
            record_data.skip(JitDumpRecordHeader::SIZE).ok()?;
            let record =
                SamplyJitCodeDebugInfo2Record::parse(self.index.endian, record_data).ok()?;
            JitDumpDebugInfoEnum::V2(record)
        } else if let Some((record_start, record_len)) = desc.code_debug_info_record_offset_and_len
        {
            let record_data = self
                .data
                .read_bytes_at(record_start, record_len.into())
                .ok()?;
            let mut record_data = RawData::Single(record_data);
            record_data.skip(JitDumpRecordHeader::SIZE).ok()?;
            let record = JitCodeDebugInfoRecord::parse(self.index.endian, record_data).ok()?;
            JitDumpDebugInfoEnum::V1(record)
        } else {
            return None;
        };
        Some(entry.insert(record))
    }
}

impl<'a> RawDataSymbolMapStringInterner<'a> {
    pub fn intern_string(&mut self, s: RawData<'a>) -> SymbolMapStringHandle {
        match s.as_slice() {
            Cow::Borrowed(s) => self.0.intern_cow(String::from_utf8_lossy(s)),
            Cow::Owned(s) => self.0.intern_owned(&String::from_utf8_lossy(&s)),
        }
    }
}

impl<'a, T: FileContents> JitDumpSymbolMapInner<'a, T> {
    fn lookup_by_entry_index(
        &self,
        index: usize,
        symbol_address: u32,
        offset_relative_to_symbol: u32,
    ) -> Option<SyncAddressInfo> {
        let mut cache = self.cache.lock().unwrap();
        let JitDumpSymbolMapCache {
            inner: cache_inner,
            string_interner,
        } = &mut *cache;

        let name_bytes = cache_inner.get_function_name(index)?;
        let name_str = String::from_utf8_lossy(name_bytes);
        let frames = Self::get_frames(
            cache_inner,
            string_interner,
            index,
            &name_str,
            offset_relative_to_symbol,
        );

        let name = string_interner.0.intern_cow(name_str);
        let symbol = SymbolInfo {
            address: symbol_address,
            size: Some(self.index.entries[index].code_bytes_len as u32),
            name: name.into(),
        };

        Some(SyncAddressInfo { symbol, frames })
    }

    fn get_frames(
        cache: &mut JitDumpSymbolMapCacheInner<'a, T>,
        string_interner: &mut RawDataSymbolMapStringInterner<'a>,
        index: usize,
        name_str: &str,
        offset_relative_to_symbol: u32,
    ) -> Option<FramesLookupResult> {
        let debug_info_v1 = match cache.get_debug_info(index) {
            // No inline info.
            Some(JitDumpDebugInfoEnum::V1(debug_info_v1)) => Some(debug_info_v1),
            // V2 has inline info.
            Some(JitDumpDebugInfoEnum::V2(debug_info_v2)) => {
                match Self::get_frames_from_debug_info_v2(
                    debug_info_v2,
                    string_interner,
                    offset_relative_to_symbol,
                ) {
                    Some(frames) => return Some(FramesLookupResult::Available(frames)),
                    None => None,
                }
            }
            // No debug info
            None => None,
        };

        let mut frame = FrameDebugInfo::default();
        let mut has_any_frame_info = false;
        if let Some(debug_info) = debug_info_v1 {
            has_any_frame_info = true;
            let lookup_avma = debug_info.code_addr + u64::from(offset_relative_to_symbol);
            let entry = debug_info.lookup(lookup_avma)?;
            let line = entry.line;
            let column = entry.column;
            let file_path = string_interner.intern_string(entry.file_path);
            let name = string_interner.0.intern_owned(name_str);
            frame.function = Some(name.into());
            frame.file_path = Some(file_path.into());
            frame.line_number = Some(line);
            frame.column_number = Some(column);
        }

        if let Some((function_name_str, raw_filename)) = parse_js_jit_function_name(name_str) {
            has_any_frame_info = true;
            let function_name = string_interner.0.intern_owned(function_name_str);
            frame.function = Some(function_name.into());
            if let Some(raw_filename) = raw_filename {
                let file_info = extract_file_info(raw_filename);
                let file = string_interner.0.intern_owned(file_info.filename);
                frame.file_path = Some(file.into());
                frame.function_start_line = file_info.start_line;
                frame.function_start_column = file_info.start_col;
            }
        }

        if has_any_frame_info {
            Some(FramesLookupResult::Available(vec![frame]))
        } else {
            None
        }
    }

    fn get_frames_from_debug_info_v2(
        debug_info: &SamplyJitCodeDebugInfo2Record<'a>,
        string_interner: &mut RawDataSymbolMapStringInterner<'a>,
        offset_relative_to_symbol: u32,
    ) -> Option<Vec<FrameDebugInfo>> {
        let index = match debug_info
            .code_offsets
            .binary_search_by_key(&offset_relative_to_symbol, |r| r.code_offset)
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let mut source_pos_index = debug_info.code_offsets.get(index).unwrap().source_position;
        let mut frames = Vec::new();
        loop {
            let Some(source_pos) = usize::try_from(source_pos_index)
                .ok()
                .and_then(|i| debug_info.source_positions.get(i))
            else {
                break;
            };
            let func_name_index = usize::try_from(source_pos.function_name).ok();
            let file_path_index = usize::try_from(source_pos.file).ok();
            let caller_index = u32::try_from(source_pos.caller_index).ok();
            let func_name = func_name_index.and_then(|i| debug_info.get_string(i));
            let file_path = file_path_index.and_then(|i| debug_info.get_string(i));

            let func_name = func_name.map(|s| string_interner.intern_string(s).into());
            let file_path = file_path.map(|s| string_interner.intern_string(s).into());
            frames.push(FrameDebugInfo {
                function: func_name,
                file_path,
                line_number: some_if_nonzero(source_pos.line),
                column_number: some_if_nonzero(source_pos.column),
                function_start_line: some_if_nonzero(source_pos.function_start_line),
                function_start_column: some_if_nonzero(source_pos.function_start_column),
            });
            if let Some(caller_index) = caller_index {
                source_pos_index = caller_index;
            } else {
                break;
            }
        }
        Some(frames)
    }
}

fn some_if_nonzero(x: u32) -> Option<u32> {
    if x != 0 {
        Some(x)
    } else {
        None
    }
}

impl<T: FileContents> SymbolMapTrait for JitDumpSymbolMapInner<'_, T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.index.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.index.relative_addresses.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let iter = (0..self.symbol_count()).filter_map(move |i| {
            let address = self.index.relative_addresses[i];
            let mut cache = self.cache.lock().unwrap();
            let name = cache.inner.get_function_name(i)?;
            Some((address, String::from_utf8_lossy(name)))
        });
        Box::new(iter)
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        let (index, symbol_address, offset_from_symbol) = match address {
            LookupAddress::Relative(address) => self.index.lookup_relative_address(address)?,
            LookupAddress::Svma(_) => {
                // SVMAs are not meaningful for JitDump files.
                return None;
            }
            LookupAddress::FileOffset(offset) => self.index.lookup_offset(offset)?,
        };
        self.lookup_by_entry_index(index, symbol_address, offset_from_symbol)
    }

    fn resolve_function_name(&self, handle: FunctionNameHandle) -> Cow<'_, str> {
        let s = self
            .cache
            .lock()
            .unwrap()
            .string_interner
            .0
            .resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str> {
        let s = self
            .cache
            .lock()
            .unwrap()
            .string_interner
            .0
            .resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        let s = self
            .cache
            .lock()
            .unwrap()
            .string_interner
            .0
            .resolve(handle.into());
        SourceFilePath::RawPath(s.expect("unknown handle?"))
    }
}
