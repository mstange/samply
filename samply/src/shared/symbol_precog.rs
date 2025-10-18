use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::{borrow::Cow, fs::File};

use serde::de::{Deserialize, Deserializer};
use serde_derive::Deserialize;
use wholesym::{SourceFilePath, SourceFilePathHandle, SourceFilePathIndex, SymbolMapGeneration};

#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct StringTableIndex(usize);

impl<'de> Deserialize<'de> for StringTableIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = usize::deserialize(deserializer)?;
        Ok(StringTableIndex(value))
    }
}

// so many string tables, none of them convenient
struct StringTable {
    strings: Vec<String>,
    generation: SymbolMapGeneration,
}

impl StringTable {
    fn get(&self, index: StringTableIndex) -> &str {
        &self.strings[index.0]
    }
}

impl<'de> Deserialize<'de> for StringTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        let generation = SymbolMapGeneration::new();
        Ok(StringTable {
            strings,
            generation,
        })
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
pub struct PrecogLibrarySymbols {
    pub debug_name: String,
    pub debug_id: String,
    pub code_id: String,
    symbol_table: Vec<InternedSymbolInfo>,

    /// Vector of (rva, index in symbol_table) so that multiple addresses
    /// within a function can share symbol info.
    ///
    /// Sorted by rva.
    known_addresses: Vec<(u32, usize)>,

    #[serde(skip)]
    string_table: Option<Arc<StringTable>>,
}

pub struct PrecogSymbolInfo {
    data: Vec<PrecogLibrarySymbols>,
}

impl<'de> Deserialize<'de> for PrecogSymbolInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PrecogSymbolInfoVisitor;

        impl<'de> serde::de::Visitor<'de> for PrecogSymbolInfoVisitor {
            type Value = PrecogSymbolInfo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct PrecogSymbolInfo")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut string_table: Option<StringTable> = None;
                let mut data: Option<Vec<PrecogLibrarySymbols>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "string_table" => {
                            string_table = Some(map.next_value()?);
                        }
                        "data" => {
                            data = Some(map.next_value()?);
                        }
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                // Give the shared string table to each PrecogLibrarySymbols
                let (string_table, mut data) = (string_table.unwrap(), data.unwrap());
                let string_table = Arc::new(string_table);
                for lib in &mut data {
                    lib.string_table = Some(string_table.clone());
                }
                Ok(PrecogSymbolInfo { data })
            }
        }

        deserializer.deserialize_map(PrecogSymbolInfoVisitor)
    }
}

impl PrecogLibrarySymbols {
    fn get_string(&self, index: StringTableIndex) -> &str {
        self.string_table.as_ref().unwrap().get(index)
    }

    fn get_owned_string(&self, index: StringTableIndex) -> String {
        self.get_string(index).to_owned()
    }

    fn get_owned_opt_string(&self, index: Option<StringTableIndex>) -> Option<String> {
        index.map(|index| self.get_string(index).to_owned())
    }

    fn handle_for_string_index(&self, index: StringTableIndex) -> SourceFilePathHandle {
        self.string_table
            .as_ref()
            .unwrap()
            .generation
            .source_file_handle(SourceFilePathIndex(index.0 as u32))
    }

    fn string_index_for_handle(&self, handle: SourceFilePathHandle) -> StringTableIndex {
        StringTableIndex(
            self.string_table
                .as_ref()
                .unwrap()
                .generation
                .unwrap_source_file_index(handle)
                .0 as usize,
        )
    }
}

impl wholesym::samply_symbols::SymbolMapTrait for PrecogLibrarySymbols {
    fn debug_id(&self) -> debugid::DebugId {
        debugid::DebugId::from_str(&self.debug_id).expect("bad debugid")
    }

    fn symbol_count(&self) -> usize {
        // not correct but maybe it's OK
        self.known_addresses.len()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, std::borrow::Cow<'_, str>)> + '_> {
        let iter = self.symbol_table.iter().map(move |info| {
            (
                info.rva,
                std::borrow::Cow::Borrowed(self.get_string(info.symbol)),
            )
        });

        Box::new(iter)
    }

    fn lookup_sync(&self, address: wholesym::LookupAddress) -> Option<wholesym::SyncAddressInfo> {
        match address {
            wholesym::LookupAddress::Relative(rva) => {
                let Ok(entry_index) = self.known_addresses.binary_search_by_key(&rva, |ka| ka.0)
                else {
                    return None;
                };
                let sym_index = self.known_addresses[entry_index].1;
                //eprintln!("lookup_sync: {:#x} -> {}", rva, sym_index);
                let info = &self.symbol_table[sym_index];
                Some(wholesym::SyncAddressInfo {
                    symbol: wholesym::SymbolInfo {
                        address: info.rva,
                        size: info.size,
                        name: self.get_owned_string(info.symbol),
                    },
                    frames: info.frames.as_ref().map(|frames| {
                        wholesym::FramesLookupResult::Available(
                            frames
                                .iter()
                                .map(|frame| wholesym::FrameDebugInfo {
                                    function: self.get_owned_opt_string(frame.function),
                                    file_path: frame
                                        .file
                                        .map(|file| self.handle_for_string_index(file)),
                                    line_number: frame.line,
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

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        let index = self.string_index_for_handle(handle);
        let s = self.get_string(index);
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

    pub fn into_iter(self) -> impl Iterator<Item = PrecogLibrarySymbols> {
        self.data.into_iter()
    }
}
