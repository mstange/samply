use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;

use yoke::Yoke;
use yoke_derive::Yokeable;

use super::index::{
    BreakpadFileLine, BreakpadFileOrInlineOriginListRef, BreakpadFuncSymbol,
    BreakpadFuncSymbolInfo, BreakpadIndex, BreakpadIndexCreator, BreakpadInlineOriginLine,
    BreakpadPublicSymbol, BreakpadPublicSymbolInfo, FileOrInlineOrigin, SYMBOL_ENTRY_KIND_FUNC,
    SYMBOL_ENTRY_KIND_PUBLIC,
};
use crate::breakpad::index::{Inlinee, SourceLine};
use crate::generation::SymbolMapGeneration;
use crate::source_file_path::SourceFilePathHandle;
use crate::symbol_map::{GetInnerSymbolMap, SymbolMapTrait};
use crate::{
    AccessPatternHint, Error, FileContents, FileContentsWrapper, FrameDebugInfo,
    FramesLookupResult, LookupAddress, SourceFilePath, SymbolInfo, SyncAddressInfo,
};

pub fn get_symbol_map_for_breakpad_sym<FC: FileContents + 'static>(
    file_contents: FileContentsWrapper<FC>,
    index_file_contents: Option<FileContentsWrapper<FC>>,
) -> Result<BreakpadSymbolMap<FC>, Error> {
    let outer = BreakpadSymbolMapOuter::new(file_contents, index_file_contents)?;
    let symbol_map = BreakpadSymbolMap(Yoke::attach_to_cart(Box::new(outer), |outer| {
        outer.make_symbol_map()
    }));
    Ok(symbol_map)
}

pub struct BreakpadSymbolMap<T: FileContents + 'static>(
    Yoke<BreakpadSymbolMapInnerWrapper<'static>, Box<BreakpadSymbolMapOuter<T>>>,
);

impl<T: FileContents> GetInnerSymbolMap for BreakpadSymbolMap<T> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a) {
        self.0.get().0.as_ref()
    }
}

enum IndexStorage<T: FileContents> {
    File(FileContentsWrapper<T>),
    Owned(Vec<u8>),
}

pub struct BreakpadSymbolMapOuter<T: FileContents> {
    data: FileContentsWrapper<T>,
    index: IndexStorage<T>,
}

impl<T: FileContents> BreakpadSymbolMapOuter<T> {
    pub fn new(
        data: FileContentsWrapper<T>,
        index_data: Option<FileContentsWrapper<T>>,
    ) -> Result<Self, Error> {
        let index = Self::make_index_storage(&data, index_data)?;
        Ok(Self { data, index })
    }

    fn make_index_storage(
        data: &FileContentsWrapper<T>,
        index_data: Option<FileContentsWrapper<T>>,
    ) -> Result<IndexStorage<T>, Error> {
        if let Some(index_data) = index_data {
            if BreakpadIndex::parse_symindex_file(&index_data).is_ok() {
                return Ok(IndexStorage::File(index_data));
            }
        }
        const CHUNK_SIZE: u64 = 1024 * 1024; // 1MB
        let mut buffer = Vec::with_capacity(CHUNK_SIZE as usize);
        let mut index_parser = BreakpadIndexCreator::new();

        // Read the entire thing in chunks to build an index.
        let len = data.len();
        let mut offset = 0;
        while offset < len {
            let chunk_len = CHUNK_SIZE.min(len - offset);
            data.read_bytes_into(&mut buffer, offset, chunk_len as usize)
                .map_err(|e| {
                    Error::HelperErrorDuringFileReading(
                        "BreakpadBreakpadSymbolMapData".to_string(),
                        e,
                    )
                })?;
            index_parser.consume(&buffer);
            buffer.clear();
            offset += CHUNK_SIZE;
        }
        let index_bytes = index_parser.finish()?;
        Ok(IndexStorage::Owned(index_bytes))
    }

    pub fn make_symbol_map(&self) -> BreakpadSymbolMapInnerWrapper<'_> {
        let index = match &self.index {
            IndexStorage::File(index_data) => {
                BreakpadIndex::parse_symindex_file(index_data).unwrap()
            }
            IndexStorage::Owned(index_data) => {
                BreakpadIndex::parse_symindex_file(&index_data[..]).unwrap()
            }
        };
        let cache = Mutex::new(BreakpadSymbolMapCache::new(&self.data, index.clone()));
        let inner_impl = BreakpadSymbolMapInner {
            data: &self.data,
            index,
            cache,
            generation: SymbolMapGeneration::new(),
        };
        BreakpadSymbolMapInnerWrapper(Box::new(inner_impl))
    }
}

#[derive(Yokeable)]
pub struct BreakpadSymbolMapInnerWrapper<'a>(Box<dyn SymbolMapTrait + Send + Sync + 'a>);

struct BreakpadSymbolMapInner<'a, T: FileContents> {
    data: &'a FileContentsWrapper<T>,
    index: BreakpadIndex<'a>,
    cache: Mutex<BreakpadSymbolMapCache<'a, T>>,
    generation: SymbolMapGeneration,
}

#[derive(Debug)]
struct BreakpadSymbolMapCache<'a, T: FileContents> {
    files: ItemCache<'a, BreakpadFileLine, T>,
    inline_origins: ItemCache<'a, BreakpadInlineOriginLine, T>,
    symbols: BreakpadSymbolMapSymbolCache<'a>,
}

#[derive(Debug, Clone, Default)]
struct BreakpadSymbolMapSymbolCache<'a> {
    public_symbols: HashMap<u64, BreakpadPublicSymbolInfo<'a>>,
    func_symbols: HashMap<u64, BreakpadFuncSymbolInfo<'a>>,
    lines: Vec<SourceLine>,
    inlinees: Vec<Inlinee>,
    only_store_latest: bool,
}

impl<'a, T: FileContents> BreakpadSymbolMapCache<'a, T> {
    pub fn new(data: &'a FileContentsWrapper<T>, index: BreakpadIndex<'a>) -> Self {
        Self {
            files: ItemCache::new(index.files, data),
            inline_origins: ItemCache::new(index.inline_origins, data),
            symbols: BreakpadSymbolMapSymbolCache::default(),
        }
    }
}

impl<'a> BreakpadSymbolMapSymbolCache<'a> {
    pub fn get_public_info<T: FileContents>(
        &mut self,
        file_offset: u64,
        line_length: u32,
        data: &'a FileContentsWrapper<T>,
    ) -> Result<BreakpadPublicSymbolInfo<'a>, Error> {
        if let Some(info) = self.public_symbols.get(&file_offset) {
            return Ok(*info);
        }

        self.clear_if_saving_memory();

        let line = data
            .read_bytes_at(file_offset, line_length.into())
            .map_err(|e| {
                Error::HelperErrorDuringFileReading("Breakpad PUBLIC symbol".to_string(), e)
            })?;
        let info = BreakpadPublicSymbol::parse(line)?;
        self.public_symbols.insert(file_offset, info);

        Ok(info)
    }

    pub fn get_func_info<'s, T: FileContents>(
        &'s mut self,
        file_offset: u64,
        block_length: u32,
        data: &'a FileContentsWrapper<T>,
    ) -> Result<BreakpadFuncSymbolInfo<'a>, Error> {
        if let Some(info) = self.func_symbols.get(&file_offset) {
            return Ok(*info);
        }

        self.clear_if_saving_memory();

        let block = data
            .read_bytes_at(file_offset, block_length.into())
            .map_err(|e| {
                Error::HelperErrorDuringFileReading("Breakpad FUNC symbol".to_string(), e)
            })?;
        let info = BreakpadFuncSymbol::parse(block, &mut self.lines, &mut self.inlinees)?;
        self.func_symbols.insert(file_offset, info);
        Ok(info)
    }

    fn clear_if_saving_memory(&mut self) {
        if self.only_store_latest {
            self.public_symbols.clear();
            self.func_symbols.clear();
            self.inlinees.clear();
            self.lines.clear();
        }
    }
}

#[derive(Debug)]
struct ItemCache<'a, I: FileOrInlineOrigin, T: FileContents> {
    item_strings: HashMap<u32, &'a str>,
    item_map: BreakpadFileOrInlineOriginListRef<'a>,
    data: &'a FileContentsWrapper<T>,
    _phantom: PhantomData<I>,
}

impl<'a, I: FileOrInlineOrigin, T: FileContents> ItemCache<'a, I, T> {
    pub fn new(
        item_map: BreakpadFileOrInlineOriginListRef<'a>,
        data: &'a FileContentsWrapper<T>,
    ) -> Self {
        Self {
            item_strings: HashMap::new(),
            item_map,
            data,
            _phantom: PhantomData,
        }
    }

    pub fn get_string(&mut self, index: u32) -> Result<&'a str, Error> {
        match self.item_strings.entry(index) {
            Entry::Occupied(name) => Ok(name.get()),
            Entry::Vacant(vacant) => {
                let entry = self
                    .item_map
                    .get(index)
                    .ok_or(Error::InvalidFileOrInlineOriginIndexInBreakpadFile(index))?;
                let file_offset = entry.offset.get();
                let line_length = entry.line_len.get();
                let line = self
                    .data
                    .read_bytes_at(file_offset, line_length.into())
                    .map_err(|e| {
                        Error::HelperErrorDuringFileReading(
                            "Breakpad FILE or INLINE_ORIGIN record".to_string(),
                            e,
                        )
                    })?;
                let s = I::parse(line)?;
                Ok(vacant.insert(s))
            }
        }
    }
}

impl<'object, T: FileContents> SymbolMapTrait for BreakpadSymbolMapInner<'object, T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.index.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.index.symbol_addresses.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let iter = (0..self.symbol_count()).filter_map(move |i| {
            let address = self.index.symbol_addresses[i].get();
            let mut cache = self.cache.lock().unwrap();
            let entry = &self.index.symbol_entries[i];
            let kind = entry.kind.get();
            let offset = entry.offset.get();
            let line_or_block_len = entry.line_or_block_len.get();
            let name = match kind {
                SYMBOL_ENTRY_KIND_PUBLIC => {
                    let public_info = cache
                        .symbols
                        .get_public_info(offset, line_or_block_len, self.data)
                        .ok()?;
                    public_info.name
                }
                SYMBOL_ENTRY_KIND_FUNC => {
                    let func_info = cache
                        .symbols
                        .get_func_info(offset, line_or_block_len, self.data)
                        .ok()?;
                    func_info.name
                }
                _ => return None,
            };
            Some((address, Cow::Borrowed(name)))
        });
        Box::new(iter)
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        let address = match address {
            LookupAddress::Relative(relative_address) => relative_address,
            LookupAddress::Svma(_) => {
                // Breakpad symbol files have no information about the image base address.
                return None;
            }
            LookupAddress::FileOffset(_) => {
                // Breakpad symbol files have no information about file offsets.
                return None;
            }
        };
        let index = match self
            .index
            .symbol_addresses
            .binary_search_by_key(&address, |a| a.get())
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_address = self.index.symbol_addresses[index].get();
        let next_symbol_address = self.index.symbol_addresses.get(index + 1).map(|a| a.get());
        let mut cache = self.cache.lock().unwrap();
        let BreakpadSymbolMapCache {
            inline_origins,
            symbols,
            ..
        } = &mut *cache;
        let entry = &self.index.symbol_entries[index];
        let kind = entry.kind.get();
        let offset = entry.offset.get();
        let line_or_block_len = entry.line_or_block_len.get();
        match kind {
            SYMBOL_ENTRY_KIND_PUBLIC => {
                let info = symbols
                    .get_public_info(offset, line_or_block_len, self.data)
                    .ok()?;
                Some(SyncAddressInfo {
                    symbol: SymbolInfo {
                        address: symbol_address,
                        size: next_symbol_address.and_then(|next_symbol_address| {
                            next_symbol_address.checked_sub(symbol_address)
                        }),
                        name: info.name.to_string(),
                    },
                    frames: None,
                })
            }
            SYMBOL_ENTRY_KIND_FUNC => {
                let info = symbols
                    .get_func_info(offset, line_or_block_len, self.data)
                    .ok()?;
                let symbols = &*symbols;
                let func_end_addr = symbol_address + info.size;
                if address >= func_end_addr {
                    return None;
                }

                let mut frames = Vec::new();
                let mut depth = 0;
                let mut name = Some(info.name);
                while let Some(inlinee) =
                    info.get_inlinee_at_depth(depth, address, &symbols.inlinees)
                {
                    frames.push(FrameDebugInfo {
                        function: name.map(ToString::to_string),
                        file_path: Some(self.generation.source_file_handle(inlinee.call_file)),
                        line_number: Some(inlinee.call_line),
                    });
                    let inline_origin = inline_origins.get_string(inlinee.origin_id).ok();
                    name = inline_origin;
                    depth += 1;
                }
                let line_info = info.get_innermost_sourceloc(address, &symbols.lines);
                let (file, line_number) = if let Some(line_info) = line_info {
                    let file = Some(self.generation.source_file_handle(line_info.file));
                    (file, Some(line_info.line))
                } else {
                    (None, None)
                };
                frames.push(FrameDebugInfo {
                    function: name.map(ToString::to_string),
                    file_path: file,
                    line_number,
                });
                frames.reverse();

                Some(SyncAddressInfo {
                    symbol: SymbolInfo {
                        address: symbol_address,
                        size: Some(info.size),
                        name: info.name.to_string(),
                    },
                    frames: Some(FramesLookupResult::Available(frames)),
                })
            }
            _ => None,
        }
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'object> {
        let index = self.generation.unwrap_source_file_index(handle);
        let mut cache = self.cache.lock().unwrap();
        let s = cache.files.get_string(index.0).ok().unwrap_or("<missing>");
        SourceFilePath::BreakpadSpecialPathStr(Cow::Borrowed(s))
    }

    fn set_access_pattern_hint(&self, hint: AccessPatternHint) {
        let mut cache = self.cache.lock().unwrap();
        cache.symbols.only_store_latest = hint == AccessPatternHint::SequentialLookup;
    }
}

#[cfg(test)]
mod test {
    use debugid::DebugId;

    use super::*;

    #[test]
    fn overeager_demangle() {
        let sym = b"MODULE Linux x86_64 BE4E976C325246EE9D6B7847A670B2A90 example-linux\nFILE 0 filename\nFUNC 1160 45 0 f\n1160 c 16 0";
        let fc = FileContentsWrapper::new(&sym[..]);
        let symbol_map = get_symbol_map_for_breakpad_sym(fc, None).unwrap();
        assert_eq!(
            symbol_map
                .get_inner_symbol_map()
                .lookup_sync(LookupAddress::Relative(0x1160))
                .unwrap()
                .symbol
                .name,
            "f"
        );
    }

    #[test]
    fn lookup_with_index() {
        // This test simulates the case where an index is created independently, for
        // example during sym file download, and then supplied as a separate file.
        let data_slices: &[&[u8]] = &[
            b"MODULE windows x86_64 F1E853FD662672044C4C44205044422E1 firefox.pdb\nIN",
            b"FO CODE_ID 63C036DBA7000 firefox.exe\nINFO GENERATOR mozilla/dump_syms ",
            b"2.1.1\nFILE 0 /builds/worker/workspace/obj-build/browser/app/d:/agent/_",
            b"work/2/s/src/vctools/delayimp/dloadsup.h\nFILE 1 /builds/worker/workspa",
            b"ce/obj-build/browser/app/d:/agent/_work/2/s/src/externalapis/windows/10",
            b"/sdk/inc/winnt.h\nINLINE_ORIGIN 0 DloadLock()\nINLINE_ORIGIN 1 DloadUnl",
            b"ock()\nINLINE_ORIGIN 2 WritePointerRelease(void**, void*)\nINLINE_ORIGI",
            b"N 3 WriteRelease64(long long*, long long)\nFUNC 2b754 aa 0 DloadAcquire",
            b"SectionWriteAccess()\nINLINE 0 658 0 0 2b76a 3d\nINLINE 0 665 0 1 2b7ca",
            b" 17 2b7e6 12\nINLINE 1 345 0 2 2b7ed b\nINLINE 2 8358 1 3 2b7ed b\n2b75",
            b"4 6 644 0\n2b75a 10 650 0\n2b76a e 299 0\n2b778 14 300 0\n2b78c 2 301 0",
            b"\n2b78e 2 306 0\n2b790 c 305 0\n2b79c b 309 0\n2b7a7 10 660 0\n2b7b7 2 ",
            b"661 0\n2b7b9 11 662 0\n2b7ca 9 340 0\n2b7d3 e 341 0\n2b7e1 c 668 0\n2b7",
            b"ed b 7729 1\n2b7f8 6 668 0",
        ];
        let mut parser = BreakpadIndexCreator::new();
        for s in data_slices {
            parser.consume(s);
        }
        let index_bytes = parser.finish().unwrap();

        let full_sym_contents = data_slices.concat();
        let sym_fc = FileContentsWrapper::new(full_sym_contents);
        let symindex_fc = FileContentsWrapper::new(index_bytes);
        let symbol_map = get_symbol_map_for_breakpad_sym(sym_fc, Some(symindex_fc)).unwrap();

        assert_eq!(
            symbol_map.get_inner_symbol_map().debug_id(),
            DebugId::from_breakpad("F1E853FD662672044C4C44205044422E1").unwrap()
        );

        let lookup_result = symbol_map
            .get_inner_symbol_map()
            .lookup_sync(LookupAddress::Relative(0x2b7ed))
            .unwrap();
        assert_eq!(
            lookup_result.symbol.name,
            "DloadAcquireSectionWriteAccess()"
        );
        assert_eq!(lookup_result.symbol.address, 0x2b754);
        assert_eq!(lookup_result.symbol.size, Some(0xaa));

        let frames = match lookup_result.frames {
            Some(FramesLookupResult::Available(frames)) => frames,
            _ => panic!("Frames should be available"),
        };
        assert_eq!(frames.len(), 4);
        assert_eq!(
            frames[0].function.as_deref().unwrap(),
            "WriteRelease64(long long*, long long)"
        );
        assert_eq!(
            symbol_map.get_inner_symbol_map().resolve_source_file_path(frames[0].file_path.unwrap()).raw_path(),
            "/builds/worker/workspace/obj-build/browser/app/d:/agent/_work/2/s/src/externalapis/windows/10/sdk/inc/winnt.h"
        );
        assert_eq!(frames[0].line_number, Some(7729));

        assert_eq!(
            frames[1].function.as_deref().unwrap(),
            "WritePointerRelease(void**, void*)"
        );
        assert_eq!(
            symbol_map.get_inner_symbol_map().resolve_source_file_path(frames[1].file_path.unwrap()).raw_path(),
            "/builds/worker/workspace/obj-build/browser/app/d:/agent/_work/2/s/src/externalapis/windows/10/sdk/inc/winnt.h"
        );
        assert_eq!(frames[1].line_number, Some(8358));

        assert_eq!(frames[2].function.as_deref().unwrap(), "DloadUnlock()");
        assert_eq!(
            symbol_map.get_inner_symbol_map().resolve_source_file_path(frames[2].file_path.unwrap()).raw_path(),
            "/builds/worker/workspace/obj-build/browser/app/d:/agent/_work/2/s/src/vctools/delayimp/dloadsup.h"
        );
        assert_eq!(frames[2].line_number, Some(345));

        assert_eq!(
            frames[3].function.as_deref().unwrap(),
            "DloadAcquireSectionWriteAccess()"
        );
        assert_eq!(
            symbol_map.get_inner_symbol_map().resolve_source_file_path(frames[3].file_path.unwrap()).raw_path(),
            "/builds/worker/workspace/obj-build/browser/app/d:/agent/_work/2/s/src/vctools/delayimp/dloadsup.h"
        );
        assert_eq!(frames[3].line_number, Some(665));
    }
}
