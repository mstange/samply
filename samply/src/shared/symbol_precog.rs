use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::{borrow::Cow, fs::File};

use serde::de::{Deserialize, Deserializer};
use serde_derive::Deserialize;
use wholesym::{
    FunctionNameIndex, SourceFilePath, SourceFilePathHandle, SourceFilePathIndex,
    SymbolMapGeneration, SymbolNameIndex,
};

#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct StringTableIndex(u32);

impl From<FunctionNameIndex> for StringTableIndex {
    fn from(value: FunctionNameIndex) -> Self {
        Self(value.0)
    }
}

impl From<SymbolNameIndex> for StringTableIndex {
    fn from(value: SymbolNameIndex) -> Self {
        Self(value.0)
    }
}

impl From<SourceFilePathIndex> for StringTableIndex {
    fn from(value: SourceFilePathIndex) -> Self {
        Self(value.0)
    }
}

impl From<StringTableIndex> for FunctionNameIndex {
    fn from(value: StringTableIndex) -> Self {
        Self(value.0)
    }
}

impl From<StringTableIndex> for SymbolNameIndex {
    fn from(value: StringTableIndex) -> Self {
        Self(value.0)
    }
}

impl From<StringTableIndex> for SourceFilePathIndex {
    fn from(value: StringTableIndex) -> Self {
        Self(value.0)
    }
}

impl<'de> Deserialize<'de> for StringTableIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        Ok(StringTableIndex(value))
    }
}

// so many string tables, none of them convenient
struct StringTable {
    strings: Vec<String>,
}

impl StringTable {
    fn get(&self, index: StringTableIndex) -> &str {
        &self.strings[index.0 as usize]
    }
}

impl<'de> Deserialize<'de> for StringTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        Ok(StringTable { strings })
    }
}

#[derive(Clone, Deserialize, Hash, PartialEq, Eq)]
struct InternedFrameDebugInfo {
    function: Option<StringTableIndex>,
    file: Option<StringTableIndex>,
    line: Option<u32>,
}

#[derive(Clone, Deserialize, Hash, PartialEq, Eq)]
struct InternedSymbolInfo {
    rva: u32,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u32>,

    symbol: StringTableIndex,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    frames: Option<Vec<InternedFrameDebugInfo>>,
}

#[derive(Clone, Deserialize)]
struct PrecogLibrarySymbolData {
    debug_name: String,
    debug_id: String,
    code_id: String,
    symbol_table: Vec<InternedSymbolInfo>,

    /// Vector of (rva, index in symbol_table) so that multiple addresses
    /// within a function can share symbol info.
    ///
    /// Sorted by rva.
    known_addresses: Vec<(u32, usize)>,
}

#[derive(Deserialize)]
pub struct PrecogSymbolInfo {
    data: Vec<PrecogLibrarySymbolData>,
    string_table: StringTable,
}

pub struct PrecogLibraySymbolMap {
    data: PrecogLibrarySymbolData,
    string_table: Arc<StringTable>,
    generation: SymbolMapGeneration,
}

impl PrecogLibraySymbolMap {
    fn new(data: PrecogLibrarySymbolData, string_table: Arc<StringTable>) -> Self {
        Self {
            data,
            string_table,
            generation: SymbolMapGeneration::new(),
        }
    }

    pub fn library_info(&self) -> wholesym::LibraryInfo {
        wholesym::LibraryInfo {
            debug_name: Some(self.data.debug_name.clone()),
            debug_id: Some(debugid::DebugId::from_str(&self.data.debug_id).unwrap()),
            code_id: wholesym::CodeId::from_str(&self.data.code_id).ok(),
            ..wholesym::LibraryInfo::default()
        }
    }
}

impl wholesym::samply_symbols::SymbolMapTrait for PrecogLibraySymbolMap {
    fn debug_id(&self) -> debugid::DebugId {
        debugid::DebugId::from_str(&self.data.debug_id).expect("bad debugid")
    }

    fn symbol_count(&self) -> usize {
        // not correct but maybe it's OK
        self.data.known_addresses.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, std::borrow::Cow<'_, str>)> + '_> {
        let iter = self.data.symbol_table.iter().map(move |info| {
            (
                info.rva,
                std::borrow::Cow::Borrowed(self.string_table.get(info.symbol)),
            )
        });

        Box::new(iter)
    }

    fn lookup_sync(&self, address: wholesym::LookupAddress) -> Option<wholesym::SyncAddressInfo> {
        match address {
            wholesym::LookupAddress::Relative(rva) => {
                let Ok(entry_index) = self
                    .data
                    .known_addresses
                    .binary_search_by_key(&rva, |ka| ka.0)
                else {
                    return None;
                };
                let sym_index = self.data.known_addresses[entry_index].1;
                //eprintln!("lookup_sync: {:#x} -> {}", rva, sym_index);
                let info = &self.data.symbol_table[sym_index];
                Some(wholesym::SyncAddressInfo {
                    symbol: wholesym::SymbolInfo {
                        address: info.rva,
                        size: info.size,
                        name: self.generation.symbol_name_handle(info.symbol.into()),
                    },
                    frames: info.frames.as_ref().map(|frames| {
                        wholesym::FramesLookupResult::Available(
                            frames
                                .iter()
                                .map(|frame| wholesym::FrameDebugInfo {
                                    function: frame
                                        .function
                                        .map(|f| self.generation.function_name_handle(f.into())),
                                    file_path: frame.file.map(|file| {
                                        self.generation.source_file_handle(file.into())
                                    }),
                                    line_number: frame.line,
                                    ..Default::default()
                                })
                                .collect(),
                        )
                    }),
                })
            }
            wholesym::LookupAddress::Svma(_) => None,
            wholesym::LookupAddress::FileOffset(_) => None,
        }
    }

    fn resolve_function_name(&self, handle: wholesym::FunctionNameHandle) -> Cow<'_, str> {
        let index = self.generation.unwrap_function_name_index(handle).into();
        let s = self.string_table.get(index);
        Cow::Borrowed(s)
    }

    fn resolve_symbol_name(&self, handle: wholesym::SymbolNameHandle) -> Cow<'_, str> {
        let index = self.generation.unwrap_symbol_name_index(handle).into();
        let s = self.string_table.get(index);
        Cow::Borrowed(s)
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        let index = self.generation.unwrap_source_file_index(handle).into();
        let s = self.string_table.get(index);
        SourceFilePath::RawPath(Cow::Borrowed(s))
    }
}

impl std::fmt::Debug for PrecogSymbolInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrecogSymbolInfo")
    }
}

impl PrecogSymbolInfo {
    pub fn try_load(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let reader = std::io::BufReader::new(file);
        serde_json::from_reader(reader).expect("failed to parse sidecar syms.json")
    }

    pub fn into_iter(self) -> impl Iterator<Item = PrecogLibraySymbolMap> {
        let Self { data, string_table } = self;
        let string_table = Arc::new(string_table);
        data.into_iter()
            .map(move |lib_data| PrecogLibraySymbolMap::new(lib_data, string_table.clone()))
    }
}
