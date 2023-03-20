use debugid::DebugId;
use linux_perf_data::jitdump::{
    JitCodeDebugInfoRecord, JitCodeLoadRecord, JitDumpReader, JitDumpRecord, JitDumpRecordHeader,
    JitDumpRecordType,
};
use linux_perf_data::{linux_perf_event_reader::RawData, Endianness};
use yoke::Yoke;

use std::{
    borrow::Cow,
    collections::{hash_map::Entry, HashMap},
    sync::Mutex,
};

use crate::{
    symbol_map::{SymbolMapInnerWrapper, SymbolMapTrait},
    AddressInfo, Error, FileContents, FileContentsWrapper, FileLocation, FrameDebugInfo,
    FramesLookupResult, SourceFilePath, SymbolInfo, SymbolMap,
};

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
struct JitDumpIndex {
    endian: Endianness,
    entries: Vec<JitDumpIndexEntry>,
    code_byte_offsets: Vec<u64>,
    debug_id: DebugId,
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
        let mut code_byte_offsets = Vec::new();
        let mut offset_and_len_of_pending_debug_record = None;
        while let Some(record_header) = reader.next_record_header()? {
            match record_header.record_type {
                JitDumpRecordType::JIT_CODE_LOAD => {
                    // Read the full record.
                    let Some(raw_record) = reader.next_record()? else { break };
                    let JitDumpRecord::CodeLoad(record) = raw_record.parse()? else { panic!() };
                    let code_debug_info_record_offset_and_len =
                        offset_and_len_of_pending_debug_record.take();
                    entries.push(JitDumpIndexEntry {
                        code_load_record_offset: raw_record.start_offset,
                        name_len: record.function_name.len() as u32,
                        code_debug_info_record_offset_and_len,
                        code_bytes_len: record.code_bytes.len() as u64,
                    });
                    code_byte_offsets.push(
                        raw_record.start_offset
                            + record.code_bytes_offset_from_record_header_start() as u64,
                    );
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
            code_byte_offsets,
            debug_id,
        })
    }
}

#[derive(Debug, Clone)]
struct JitDumpIndexEntry {
    code_load_record_offset: u64,
    name_len: u32,
    code_debug_info_record_offset_and_len: Option<(u64, u32)>,
    code_bytes_len: u64,
}

pub fn get_symbol_map_for_jitdump<F, FL>(
    file_contents: FileContentsWrapper<F>,
    file_location: FL,
) -> Result<SymbolMap<FL>, Error>
where
    F: FileContents + 'static,
    FL: FileLocation,
{
    let outer = JitDumpSymbolMapOuter::new(file_contents)?;
    let symbol_map = JitDumpSymbolMap(Yoke::attach_to_cart(Box::new(outer), |outer| {
        outer.make_symbol_map()
    }));
    Ok(SymbolMap::new(file_location, Box::new(symbol_map)))
}

pub struct JitDumpSymbolMap<T: FileContents>(
    Yoke<SymbolMapInnerWrapper<'static>, Box<JitDumpSymbolMapOuter<T>>>,
);

impl<T: FileContents> SymbolMapTrait for JitDumpSymbolMap<T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.0.get().0.debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.0.get().0.symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.get().0.iter_symbols()
    }

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo> {
        self.0.get().0.lookup_relative_address(address)
    }

    fn lookup_svma(&self, svma: u64) -> Option<AddressInfo> {
        self.0.get().0.lookup_svma(svma)
    }

    fn lookup_offset(&self, offset: u64) -> Option<AddressInfo> {
        self.0.get().0.lookup_offset(offset)
    }
}

pub struct JitDumpSymbolMapOuter<T: FileContents> {
    data: FileContentsWrapper<T>,
    index: JitDumpIndex,
}

struct FileContentsCursor<'a, T: FileContents> {
    /// Invariant: current_offset + remaining_len == total_len
    current_offset: u64,
    /// Invariant: current_offset + remaining_len == total_len
    remaining_len: u64,
    inner: &'a FileContentsWrapper<T>,
}

impl<'a, T: FileContents> FileContentsCursor<'a, T> {
    pub fn new(inner: &'a FileContentsWrapper<T>) -> Self {
        let remaining_len = inner.len();
        Self {
            current_offset: 0,
            remaining_len,
            inner,
        }
    }
}

impl<'a, T: FileContents> std::io::Read for FileContentsCursor<'a, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read_len = <[u8]>::len(buf).min(self.remaining_len as usize);
        // Make a silly copy
        let mut tmp_buf = Vec::with_capacity(read_len);
        self.inner
            .read_bytes_into(&mut tmp_buf, self.current_offset, read_len)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        buf[..read_len].copy_from_slice(&tmp_buf);
        self.current_offset += read_len as u64;
        self.remaining_len -= read_len as u64;
        Ok(read_len)
    }
}

impl<'a, T: FileContents> std::io::Seek for FileContentsCursor<'a, T> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        /// Returns (new_offset, new_remaining_len)
        fn inner(cur: u64, total_len: u64, pos: std::io::SeekFrom) -> Option<(u64, u64)> {
            let new_offset: u64 = match pos {
                std::io::SeekFrom::Start(pos) => pos,
                std::io::SeekFrom::End(pos) => {
                    (total_len as i64).checked_add(pos)?.try_into().ok()?
                }
                std::io::SeekFrom::Current(pos) => {
                    (cur as i64).checked_add(pos)?.try_into().ok()?
                }
            };
            let new_remaining = total_len.checked_sub(new_offset)?;
            Some((new_offset, new_remaining))
        }

        let cur = self.current_offset;
        let total_len = self.current_offset + self.remaining_len;
        match inner(cur, total_len, pos) {
            Some((cur, rem)) => {
                self.current_offset = cur;
                self.remaining_len = rem;
                Ok(cur)
            }
            None => Err(std::io::Error::new(std::io::ErrorKind::Other, "Bad Seek")),
        }
    }
}

impl<T: FileContents> JitDumpSymbolMapOuter<T> {
    pub fn new(data: FileContentsWrapper<T>) -> Result<Self, Error> {
        let cursor = FileContentsCursor::new(&data);
        let reader = JitDumpReader::new(cursor)?;
        let index = JitDumpIndex::from_reader(reader).map_err(Error::JitDumpFileReading)?;
        Ok(Self { data, index })
    }

    pub fn make_symbol_map(&self) -> SymbolMapInnerWrapper<'_> {
        let inner = JitDumpSymbolMapInner {
            index: &self.index,
            cache: Mutex::new(JitDumpSymbolMapCache::new(&self.data, &self.index)),
        };
        SymbolMapInnerWrapper(Box::new(inner))
    }
}

struct JitDumpSymbolMapInner<'a, T: FileContents> {
    index: &'a JitDumpIndex,
    cache: Mutex<JitDumpSymbolMapCache<'a, T>>,
}

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

impl<'a, T: FileContents> SymbolMapTrait for JitDumpSymbolMapInner<'a, T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.index.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.index.code_byte_offsets.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let iter = (0..self.symbol_count()).filter_map(move |i| {
            let address = self.index.code_byte_offsets[i];
            let mut cache = self.cache.lock().unwrap();
            let name = cache.get_function_name(i)?;
            Some((address as u32, String::from_utf8_lossy(name)))
        });
        Box::new(iter)
    }

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo> {
        // Relative addresses and file offsets are equivalent for JitDump files.
        self.lookup_offset(address.into())
    }

    fn lookup_svma(&self, _svma: u64) -> Option<AddressInfo> {
        // SVMAs are not meaningful for JitDump files.
        None
    }

    fn lookup_offset(&self, address: u64) -> Option<AddressInfo> {
        let index = match self.index.code_byte_offsets.binary_search(&address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_address = self.index.code_byte_offsets[index];
        let desc = &self.index.entries[index];
        let offset_relative_to_symbol = address - symbol_address;
        if offset_relative_to_symbol >= desc.code_bytes_len {
            return None;
        }
        let mut cache = self.cache.lock().unwrap();
        let name_bytes = cache.get_function_name(index)?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let debug_info = cache.get_debug_info(index);
        let frames = match debug_info {
            Some(debug_info) => {
                let lookup_avma = debug_info.code_addr + offset_relative_to_symbol;
                match debug_info.lookup(lookup_avma) {
                    Some(entry) => {
                        let file_path =
                            String::from_utf8_lossy(&entry.file_path.as_slice()).into_owned();
                        let frame = FrameDebugInfo {
                            function: Some(name.clone()),
                            file_path: Some(SourceFilePath::new(file_path, None)),
                            line_number: Some(entry.line),
                        };
                        FramesLookupResult::Available(vec![frame])
                    }
                    None => FramesLookupResult::Unavailable,
                }
            }
            None => FramesLookupResult::Unavailable,
        };
        Some(AddressInfo {
            symbol: SymbolInfo {
                address: symbol_address as u32,
                size: Some(desc.code_bytes_len as u32),
                name,
            },
            frames,
        })
    }
}
