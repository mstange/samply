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
use crate::shared::{
    FileContents, FileContentsCursor, FileContentsWrapper, FrameDebugInfo, FramesLookupResult,
    LookupAddress, SourceFilePath, SymbolInfo,
};
use crate::symbol_map::{GetInnerSymbolMap, SymbolMap, SymbolMapTrait};
use crate::{FileAndPathHelper, SyncAddressInfo};

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
            cache: Mutex::new(JitDumpSymbolMapCache::new(&self.data, &self.index)),
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
    data: &'a FileContentsWrapper<T>,
    index: &'a JitDumpIndex,
}

impl<'a, T: FileContents> JitDumpSymbolMapCache<'a, T> {
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
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let debug_info = cache.get_debug_info(index);
        let frames = debug_info.and_then(|debug_info| {
            let lookup_avma = debug_info.code_addr + offset_relative_to_symbol;
            let entry = debug_info.lookup(lookup_avma)?;
            let file_path = String::from_utf8_lossy(&entry.file_path.as_slice()).into_owned();
            let frame = FrameDebugInfo {
                function: Some(name.clone()),
                file_path: Some(SourceFilePath::new(file_path, None)),
                line_number: Some(entry.line),
            };
            Some(FramesLookupResult::Available(vec![frame]))
        });
        Some(SyncAddressInfo {
            symbol: SymbolInfo {
                address: symbol_address,
                size: Some(self.index.entries[index].code_bytes_len as u32),
                name,
            },
            frames,
        })
    }
}

impl<'a, T: FileContents> SymbolMapTrait for JitDumpSymbolMapInner<'a, T> {
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
}
