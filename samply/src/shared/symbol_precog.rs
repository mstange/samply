use std::fs::File;
use std::io::BufWriter;
use std::str::FromStr;
use std::sync::Arc;
use std::{collections::HashMap, path::Path};

use debugid::DebugId;
use serde::{
    ser::SerializeMap, ser::SerializeSeq, Deserialize, Deserializer, Serialize, Serializer,
};
use serde_derive::{Deserialize, Serialize};
use serde_json::to_writer;
use wholesym::SourceFilePath;

#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct StringTableIndex(usize);

impl Serialize for StringTableIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StringTableIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = usize::deserialize(deserializer)?;
        Ok(StringTableIndex(value))
    }
}

// so many string tables, none of them convenient
struct StringTable {
    string_map: HashMap<String, usize>,
    strings: Vec<String>,
}

impl StringTable {
    fn new() -> Self {
        let mut result = Self {
            string_map: HashMap::new(),
            strings: Vec::new(),
        };
        result.intern_string("UNKNOWN"); // always at index 0
        result
    }

    fn intern_string(&mut self, string: &str) -> StringTableIndex {
        let index = match self.string_map.get(string) {
            Some(&index) => index,
            None => {
                let index = self.strings.len();
                self.strings.push(string.to_string());
                self.string_map.insert(string.to_string(), index);
                index
            }
        };
        StringTableIndex(index)
        //StringTableIndex(string.to_owned())
    }

    fn get(&self, index: StringTableIndex) -> &str {
        &self.strings[index.0]
        //&index.0
    }
}

impl Serialize for StringTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.strings.len()))?;
        for string in &self.strings {
            seq.serialize_element(string)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for StringTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        let mut string_map = HashMap::new();
        for (index, string) in strings.iter().enumerate() {
            string_map.insert(string.clone(), index);
        }
        Ok(StringTable {
            string_map,
            strings,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct InternedFrameDebugInfo {
    function: Option<StringTableIndex>,
    file: Option<StringTableIndex>,
    line: Option<u32>,
}

impl InternedFrameDebugInfo {
    fn new(frame: &wholesym::FrameDebugInfo, strtab: &mut StringTable) -> InternedFrameDebugInfo {
        let function = frame
            .function
            .as_ref()
            .map(|name| strtab.intern_string(name));
        let file = frame
            .file_path
            .as_ref()
            .map(|name| strtab.intern_string(name.raw_path()));
        InternedFrameDebugInfo {
            function,
            file,
            line: frame.line_number,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
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

impl InternedSymbolInfo {
    fn new(info: &wholesym::AddressInfo, strtab: &mut StringTable) -> InternedSymbolInfo {
        let symbol = strtab.intern_string(&info.symbol.name);
        let frames = info.frames.as_ref().map(|frames| {
            frames
                .iter()
                .map(|frame| InternedFrameDebugInfo::new(frame, strtab))
                .collect()
        });
        InternedSymbolInfo {
            rva: info.symbol.address,
            size: info.symbol.size,
            symbol,
            frames,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct PrecogLibrarySymbols {
    debug_name: String,
    debug_id: String,
    code_id: String,
    symbol_table: Vec<InternedSymbolInfo>,
    // vector of (rva, index in symbol_table) so that multiple addresses
    // within a function map to the same symbol
    known_addresses: Vec<(u32, usize)>,

    #[serde(skip)]
    string_table: Option<Arc<StringTable>>,
}

pub struct PrecogSymbolInfo {
    string_table: Arc<StringTable>,
    data: Vec<PrecogLibrarySymbols>,
}

impl Serialize for PrecogSymbolInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let string_table = self.string_table.as_ref();
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("string_table", string_table)?;
        map.serialize_entry("data", &self.data)?;
        map.end()
    }
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
                Ok(PrecogSymbolInfo { string_table, data })
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
                for (known_rva, sym_index) in &self.known_addresses {
                    if *known_rva == rva {
                        //eprintln!("lookup_sync: 0x{:x} -> {}", rva, info.symbol.0);
                        let info = &self.symbol_table[*sym_index];
                        return Some(wholesym::SyncAddressInfo {
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
                                            file_path: frame.file.map(|file| {
                                                SourceFilePath::new(
                                                    self.get_string(file).to_owned(),
                                                    None,
                                                )
                                            }),
                                            line_number: frame.line,
                                        })
                                        .collect(),
                                )
                            }),
                        });
                    }
                }
                None
            }
            wholesym::LookupAddress::Svma(_) => None,
            wholesym::LookupAddress::FileOffset(_) => None,
        }
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

    pub fn into_hash_map(
        self,
    ) -> HashMap<DebugId, Arc<dyn wholesym::samply_symbols::SymbolMapTrait + Send + Sync>> {
        self.data
            .into_iter()
            .map(|lib| {
                (
                    DebugId::from_str(&lib.debug_id).unwrap(),
                    Arc::new(lib)
                        as Arc<dyn wholesym::samply_symbols::SymbolMapTrait + Send + Sync>,
                )
            })
            .collect()
    }
}

pub fn presymbolicate(profile: &fxprof_processed_profile::Profile, precog_output: &Path) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut string_table = StringTable::new();
    let mut results = Vec::new();

    let config = wholesym::SymbolManagerConfig::new()
        .use_spotlight(true)
        // .verbose(true)
        .respect_nt_symbol_path(true);
    let mut symbol_manager = wholesym::SymbolManager::with_config(config);

    for (lib, rvas) in profile.lib_used_rva_iter() {
        // Add the library to the symbol manager with all the info, so that load_symbol_map can find it later
        symbol_manager.add_known_library(wholesym::LibraryInfo {
            name: Some(lib.debug_name.clone()),
            path: Some(lib.path.clone()),
            debug_path: Some(lib.debug_path.clone()),
            debug_id: Some(lib.debug_id),
            arch: lib.arch.clone(),
            debug_name: Some(lib.debug_name.clone()),
            code_id: lib
                .code_id
                .as_ref()
                .map(|id| wholesym::CodeId::from_str(id).expect("bad codeid")),
        });

        //eprintln!("Library {} ({}) has {} rvas", lib.debug_name, lib.debug_id, rvas.len());

        let result = rt.block_on(async {
            let Ok(symbol_map) = symbol_manager
                .load_symbol_map(&lib.debug_name, lib.debug_id)
                .await
            else {
                //eprintln!("Couldn't load symbol map for {} at {} {} ({})", lib.debug_name, lib.path, lib.debug_path, lib.debug_id);
                return None;
            };

            let mut symbol_table = Vec::new();
            let mut symbol_table_map = HashMap::new();

            let mut known_addresses = Vec::new();
            for rva in rvas {
                if let Some(addr_info) = symbol_map
                    .lookup(wholesym::LookupAddress::Relative(*rva))
                    .await
                {
                    let index = symbol_table_map
                        .entry(addr_info.symbol.address)
                        .or_insert_with(|| {
                            let info = InternedSymbolInfo::new(&addr_info, &mut string_table);
                            symbol_table.push(info);
                            symbol_table.len() - 1
                        });
                    known_addresses.push((*rva, *index));
                }
            }

            Some(PrecogLibrarySymbols {
                debug_name: lib.debug_name.clone(),
                debug_id: lib.debug_id.to_string(),
                code_id: lib
                    .code_id
                    .as_ref()
                    .map(|id| id.to_string())
                    .unwrap_or("".to_owned()),
                symbol_table,
                known_addresses,
                string_table: None,
            })
        });

        if let Some(result) = result {
            results.push(result);
        }
    }

    {
        let string_table = Arc::new(string_table);
        for lib in &mut results {
            lib.string_table = Some(string_table.clone());
        }
        let info = PrecogSymbolInfo {
            string_table,
            data: results,
        };

        let file = File::create(precog_output).unwrap();
        let writer = BufWriter::new(file);
        to_writer(writer, &info).expect("Couldn't write JSON for presymbolication");
    }
}
