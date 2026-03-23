use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;

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
        let mut offset_and_len_of_pending_debug_record = None;
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
                        offset_and_len_of_pending_debug_record.take();
                    let relative_address = cumulative_address;
                    cumulative_address += record.code_bytes.len() as u32;

                    entries.push(JitDumpIndexEntry {
                        code_load_record_offset: raw_record.start_offset,
                        code_bytes_offset: raw_record.start_offset
                            + record.code_bytes_offset_from_record_header_start() as u64,
                        name_len: record.function_name.len() as u32,
                        code_debug_info_record_offset_and_len,
                        code_bytes_len: record.code_bytes.len() as u64,
                    });
                    relative_addresses.push(relative_address);
                }
                JitDumpRecordType::JIT_CODE_DEBUG_INFO => {
                    let offset = reader.next_record_offset();
                    if !reader.skip_next_record()? {
                        break;
                    }
                    offset_and_len_of_pending_debug_record =
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
    pub fn lookup_relative_address(&self, address: u32) -> Option<(usize, u32, u64)> {
        let index = match self.relative_addresses.binary_search(&address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_address = self.relative_addresses[index];
        let offset_relative_to_symbol = (address - symbol_address) as u64;
        let desc = &self.entries[index];
        if offset_relative_to_symbol >= desc.code_bytes_len {
            return None;
        }
        Some((index, symbol_address, offset_relative_to_symbol))
    }

    /// Returns (entry index, entry relative address, offset from entry start)
    pub fn lookup_offset(&self, offset: u64) -> Option<(usize, u32, u64)> {
        let index = match self
            .entries
            .binary_search_by_key(&offset, |entry| entry.code_bytes_offset)
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_code_bytes_offset = self.entries[index].code_bytes_offset;
        let offset_relative_to_symbol = offset - symbol_code_bytes_offset;
        let desc = &self.entries[index];
        if offset_relative_to_symbol >= desc.code_bytes_len {
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
    names: HashMap<usize, &'a [u8]>,
    debug_infos: HashMap<usize, JitCodeDebugInfoRecord<'a>>,
    string_interner: SymbolMapStringInterner<'a>,
    data: &'a FileContentsWrapper<T>,
    index: &'a JitDumpIndex,
}

impl<'a, T: FileContents> JitDumpSymbolMapCache<'a, T> {
    pub fn new(
        data: &'a FileContentsWrapper<T>,
        index: &'a JitDumpIndex,
        generation: SymbolMapGeneration,
    ) -> Self {
        Self {
            names: HashMap::new(),
            debug_infos: HashMap::new(),
            string_interner: SymbolMapStringInterner::new(generation),
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

    pub fn get_debug_info(&mut self, entry_index: usize) -> Option<&JitCodeDebugInfoRecord<'a>> {
        match self.debug_infos.entry(entry_index) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let desc = &self.index.entries[entry_index];
                let (record_start, record_len) = desc.code_debug_info_record_offset_and_len?;
                let record_data = self
                    .data
                    .read_bytes_at(record_start, record_len.into())
                    .ok()?;
                let mut record_data = RawData::Single(record_data);
                record_data.skip(JitDumpRecordHeader::SIZE).ok()?;
                let record = JitCodeDebugInfoRecord::parse(self.index.endian, record_data).ok()?;
                Some(entry.insert(record))
            }
        }
    }
}

impl<'a, T: FileContents> JitDumpSymbolMapInner<'a, T> {
    fn lookup_by_entry_index(
        &self,
        index: usize,
        symbol_address: u32,
        offset_relative_to_symbol: u64,
    ) -> Option<SyncAddressInfo> {
        let mut cache = self.cache.lock().unwrap();
        let name_bytes = cache.get_function_name(index)?;
        let name_str = String::from_utf8_lossy(name_bytes);
        let frames = Self::get_frames(&mut cache, index, &name_str, offset_relative_to_symbol);

        let name = cache.string_interner.intern_cow(name_str);
        let symbol = SymbolInfo {
            address: symbol_address,
            size: Some(self.index.entries[index].code_bytes_len as u32),
            name: name.into(),
        };
        Some(SyncAddressInfo { symbol, frames })
    }

    fn get_frames(
        cache: &mut JitDumpSymbolMapCache<'a, T>,
        index: usize,
        name_str: &str,
        offset_relative_to_symbol: u64,
    ) -> Option<FramesLookupResult> {
        let mut frame = FrameDebugInfo::default();
        let mut has_any_frame_info = false;
        if let Some(debug_info) = cache.get_debug_info(index) {
            has_any_frame_info = true;
            let lookup_avma = debug_info.code_addr + offset_relative_to_symbol;
            let entry = debug_info.lookup(lookup_avma)?;
            let line = entry.line;
            let column = entry.column;
            let file_path = match entry.file_path.as_slice() {
                Cow::Borrowed(s) => cache.string_interner.intern_cow(String::from_utf8_lossy(s)),
                Cow::Owned(s) => cache
                    .string_interner
                    .intern_owned(&String::from_utf8_lossy(&s)),
            };
            let name = cache.string_interner.intern_owned(name_str);
            frame.function = Some(name.into());
            frame.file_path = Some(file_path.into());
            frame.line_number = Some(line);
            frame.column_number = Some(column);
        }

        if let Some((function_name_str, raw_filename)) = parse_js_jit_function_name(name_str) {
            has_any_frame_info = true;
            let function_name = cache.string_interner.intern_owned(function_name_str);
            frame.function = Some(function_name.into());
            if let Some(raw_filename) = raw_filename {
                let file_info = extract_file_info(raw_filename);
                let file = cache.string_interner.intern_owned(file_info.filename);
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
            let name = cache.get_function_name(i)?;
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
            .resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str> {
        let s = self
            .cache
            .lock()
            .unwrap()
            .string_interner
            .resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        let s = self
            .cache
            .lock()
            .unwrap()
            .string_interner
            .resolve(handle.into());
        SourceFilePath::RawPath(s.expect("unknown handle?"))
    }
}
