use std::{
    borrow::Cow,
    collections::{hash_map::Entry, HashMap},
    sync::Mutex,
};

use yoke::Yoke;

use crate::{
    symbol_map::{SymbolMapInnerWrapper, SymbolMapTrait},
    AddressInfo, Error, FileContents, FileContentsWrapper, FileLocation, FrameDebugInfo,
    FramesLookupResult, SourceFilePath, SymbolInfo, SymbolMap,
};

use super::index::{
    BreakpadFileLine, BreakpadFuncSymbol, BreakpadFuncSymbolInfo, BreakpadIndex,
    BreakpadIndexParser, BreakpadInlineOriginLine, BreakpadPublicSymbol, BreakpadPublicSymbolInfo,
    BreakpadSymbolType, FileOrInlineOrigin,
};

pub fn get_symbol_map_for_breakpad_sym<F, FL>(
    file_contents: FileContentsWrapper<F>,
    file_location: FL,
) -> Result<SymbolMap<FL>, Error>
where
    F: FileContents + 'static,
    FL: FileLocation,
{
    let outer = BreakpadSymbolMapOuter::new(file_contents)?;
    let symbol_map = BreakpadSymbolMap(Yoke::attach_to_cart(Box::new(outer), |outer| {
        outer.make_symbol_map()
    }));
    Ok(SymbolMap::new(file_location, Box::new(symbol_map)))
}

pub struct BreakpadSymbolMap<T: FileContents>(
    Yoke<SymbolMapInnerWrapper<'static>, Box<BreakpadSymbolMapOuter<T>>>,
);

impl<T: FileContents> SymbolMapTrait for BreakpadSymbolMap<T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.0.get().0.debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.0.get().0.symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.get().0.iter_symbols()
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.get().0.lookup(address)
    }
}

pub struct BreakpadSymbolMapOuter<T: FileContents> {
    data: FileContentsWrapper<T>,
    index: BreakpadIndex,
}

impl<T: FileContents> BreakpadSymbolMapOuter<T> {
    pub fn new(data: FileContentsWrapper<T>) -> Result<Self, Error> {
        const CHUNK_SIZE: u64 = 1024 * 1024; // 1MB
        let mut buffer = Vec::with_capacity(CHUNK_SIZE as usize);
        let mut index_parser = BreakpadIndexParser::new();

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
        let index = index_parser.finish()?;
        Ok(Self { data, index })
    }

    pub fn make_symbol_map(&self) -> SymbolMapInnerWrapper<'_> {
        let inner = BreakpadSymbolMapInner {
            data: &self.data,
            index: &self.index,
            cache: Mutex::new(BreakpadSymbolMapCache::new(&self.data, &self.index)),
        };
        SymbolMapInnerWrapper(Box::new(inner))
    }
}

struct BreakpadSymbolMapInner<'a, T: FileContents> {
    data: &'a FileContentsWrapper<T>,
    index: &'a BreakpadIndex,
    cache: Mutex<BreakpadSymbolMapCache<'a, T>>,
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
}

impl<'a, T: FileContents> BreakpadSymbolMapCache<'a, T> {
    pub fn new(data: &'a FileContentsWrapper<T>, index: &'a BreakpadIndex) -> Self {
        Self {
            files: ItemCache::new(&index.files, data),
            inline_origins: ItemCache::new(&index.inline_origins, data),
            symbols: BreakpadSymbolMapSymbolCache::default(),
        }
    }
}

impl<'a> BreakpadSymbolMapSymbolCache<'a> {
    pub fn get_public_info<'s, T: FileContents>(
        &'s mut self,
        public: &BreakpadPublicSymbol,
        data: &'a FileContentsWrapper<T>,
    ) -> Result<&'s BreakpadPublicSymbolInfo<'a>, Error> {
        match self.public_symbols.entry(public.file_offset) {
            Entry::Occupied(info) => Ok(info.into_mut()),
            Entry::Vacant(vacant) => {
                let line = data
                    .read_bytes_at(public.file_offset, public.line_length.into())
                    .map_err(|e| {
                        Error::HelperErrorDuringFileReading("Breakpad PUBLIC symbol".to_string(), e)
                    })?;
                let info = public.parse(line)?;
                Ok(vacant.insert(info))
            }
        }
    }

    pub fn get_func_info<'s, T: FileContents>(
        &'s mut self,
        func: &BreakpadFuncSymbol,
        data: &'a FileContentsWrapper<T>,
    ) -> Result<&'s BreakpadFuncSymbolInfo<'a>, Error> {
        match self.func_symbols.entry(func.file_offset) {
            Entry::Occupied(info) => Ok(info.into_mut()),
            Entry::Vacant(vacant) => {
                let block = data
                    .read_bytes_at(func.file_offset, func.block_length.into())
                    .map_err(|e| {
                        Error::HelperErrorDuringFileReading("Breakpad FUNC symbol".to_string(), e)
                    })?;
                let info = func.parse(block)?;
                Ok(vacant.insert(info))
            }
        }
    }
}

#[derive(Debug)]
struct ItemCache<'a, I: FileOrInlineOrigin, T: FileContents> {
    items: HashMap<u32, &'a str>,
    item_offsets: &'a HashMap<u32, I>,
    data: &'a FileContentsWrapper<T>,
}

impl<'a, I: FileOrInlineOrigin, T: FileContents> ItemCache<'a, I, T> {
    pub fn new(item_offsets: &'a HashMap<u32, I>, data: &'a FileContentsWrapper<T>) -> Self {
        Self {
            items: HashMap::new(),
            item_offsets,
            data,
        }
    }

    pub fn get_str(&mut self, index: u32) -> Result<&'a str, Error> {
        match self.items.entry(index) {
            Entry::Occupied(name) => Ok(name.into_mut()),
            Entry::Vacant(vacant) => {
                let offsets = self
                    .item_offsets
                    .get(&index)
                    .ok_or(Error::InvalidFileOrInlineOriginIndexInBreakpadFile(index))?;
                let (file_offset, line_length) = offsets.offset_and_length();
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

impl<'a, T: FileContents> SymbolMapTrait for BreakpadSymbolMapInner<'a, T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.index.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.index.symbol_addresses.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let iter = (0..self.symbol_count()).filter_map(move |i| {
            let address = self.index.symbol_addresses[i];
            let mut cache = self.cache.lock().unwrap();
            let name = match &self.index.symbol_offsets[i] {
                super::index::BreakpadSymbolType::Public(public) => {
                    let public_info = cache.symbols.get_public_info(public, self.data).ok()?;
                    public_info.name
                }
                super::index::BreakpadSymbolType::Func(func) => {
                    let func_info = cache.symbols.get_func_info(func, self.data).ok()?;
                    func_info.name
                }
            };
            Some((address, Cow::Borrowed(name)))
        });
        Box::new(iter)
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        let index = match self.index.symbol_addresses.binary_search(&address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let symbol_address = self.index.symbol_addresses[index];
        let next_symbol_address = self.index.symbol_addresses.get(index + 1);
        let mut cache = self.cache.lock().unwrap();
        let BreakpadSymbolMapCache {
            files,
            inline_origins,
            symbols,
        } = &mut *cache;
        match &self.index.symbol_offsets[index] {
            BreakpadSymbolType::Public(public) => {
                let info = symbols.get_public_info(public, self.data).ok()?;
                Some(AddressInfo {
                    symbol: SymbolInfo {
                        address: symbol_address,
                        size: next_symbol_address.and_then(|next_symbol_address| {
                            next_symbol_address.checked_sub(symbol_address)
                        }),
                        name: info.name.to_string(),
                    },
                    frames: FramesLookupResult::Unavailable,
                })
            }
            BreakpadSymbolType::Func(func) => {
                let info = symbols.get_func_info(func, self.data).ok()?;
                let func_end_addr = symbol_address + info.size;
                if address >= func_end_addr {
                    return None;
                }

                let mut frames = Vec::new();
                let mut depth = 0;
                let mut name = Some(info.name.to_string());
                while let Some(inlinee) = info.get_inlinee_at_depth(depth, address) {
                    let file = files
                        .get_str(inlinee.call_file)
                        .ok()
                        .map(ToString::to_string);
                    frames.push(FrameDebugInfo {
                        function: name,
                        file_path: file.map(SourceFilePath::from_breakpad_path),
                        line_number: Some(inlinee.call_line),
                    });
                    let inline_origin = inline_origins
                        .get_str(inlinee.origin_id)
                        .ok()
                        .map(ToString::to_string);
                    name = inline_origin;
                    depth += 1;
                }
                let line_info = info.get_innermost_sourceloc(address);
                let (file, line_number) = if let Some(line_info) = line_info {
                    let file = files.get_str(line_info.file).ok().map(ToString::to_string);
                    (file, Some(line_info.line))
                } else {
                    (None, None)
                };
                frames.push(FrameDebugInfo {
                    function: name,
                    file_path: file.map(SourceFilePath::from_breakpad_path),
                    line_number,
                });
                frames.reverse();

                Some(AddressInfo {
                    symbol: SymbolInfo {
                        address: symbol_address,
                        size: Some(info.size),
                        name: info.name.to_string(),
                    },
                    frames: FramesLookupResult::Available(frames),
                })
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Clone)]
    struct DummyLocation;

    impl FileLocation for DummyLocation {
        fn location_for_dyld_subcache(&self, _suffix: &str) -> Option<Self> {
            None
        }

        fn location_for_external_object_file(&self, _object_file: &str) -> Option<Self> {
            None
        }

        fn location_for_pdb_from_binary(&self, _pdb_path: &str) -> Option<Self> {
            None
        }

        fn location_for_source_file(&self, _source_file_path: &str) -> Option<Self> {
            None
        }
    }
    impl std::fmt::Display for DummyLocation {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            "DummyLocation".fmt(f)
        }
    }

    #[test]
    fn overeager_demangle() {
        let sym = b"MODULE Linux x86_64 BE4E976C325246EE9D6B7847A670B2A90 example-linux\nFILE 0 filename\nFUNC 1160 45 0 f\n1160 c 16 0";
        let fc = FileContentsWrapper::new(&sym[..]);
        let symbol_map = get_symbol_map_for_breakpad_sym(fc, DummyLocation).unwrap();
        assert_eq!(symbol_map.lookup(0x1160).unwrap().symbol.name, "f");
    }
}
