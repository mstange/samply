use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use debugid::DebugId;
use futures_util::future::join_all;
use indexmap::IndexSet;
use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_derive::{Deserialize, Serialize};
use serde_json::to_writer;
use wholesym::{SourceFilePath, SymbolManager};

use crate::symbols::create_symbol_manager_and_quota_manager;

use super::prop_types::SymbolProps;

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
        result.handle_for_string("UNKNOWN"); // always at index 0
        result
    }

    fn handle_for_string(&mut self, string: &str) -> StringTableIndex {
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

#[derive(Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
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
            .map(|name| strtab.handle_for_string(name));
        let file = frame
            .file_path
            .as_ref()
            .map(|name| strtab.handle_for_string(name.raw_path()));
        InternedFrameDebugInfo {
            function,
            file,
            line: frame.line_number,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
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
        let symbol = strtab.handle_for_string(&info.symbol.name);
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

    /// Vector of (rva, index in symbol_table) so that multiple addresses
    /// within a function can share symbol info.
    ///
    /// Sorted by rva.
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
                                    file_path: frame.file.map(|file| {
                                        SourceFilePath::new(self.get_string(file).to_owned(), None)
                                    }),
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

pub fn presymbolicate(
    profile: &fxprof_processed_profile::Profile,
    precog_output: &Path,
    symbol_props: SymbolProps,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let (mut results, string_table) = rt.block_on(async {
        let (mut symbol_manager, quota_manager) =
            create_symbol_manager_and_quota_manager(symbol_props, false);

        let native_frame_addresses_per_library = profile.native_frame_addresses_per_library();
        let lib_stuff: Vec<_> = native_frame_addresses_per_library
            .into_iter()
            .map(|(lib_handle, rvas)| {
                let lib = profile.get_library_info(lib_handle);
                let lib_info = wholesym::LibraryInfo {
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
                };
                let rvas: Vec<u32> = rvas.into_iter().collect();
                (lib_info, rvas)
            })
            .collect();

        for (lib_info, _) in &lib_stuff {
            // Add the library to the symbol manager with all the info, so that load_symbol_map can find it later
            symbol_manager.add_known_library(lib_info.clone());
        }

        let string_table = Arc::new(Mutex::new(StringTable::new()));
        let symbol_manager = Arc::new(symbol_manager);

        let symbolication_tasks = lib_stuff.into_iter().map(|(lib, rvas)| {
            let symbol_manager = Arc::clone(&symbol_manager);
            let string_table = Arc::clone(&string_table);
            tokio::spawn(async move {
                get_lib_symbols(lib, &rvas, &symbol_manager, string_table.clone()).await
            })
        });

        let symbolication_results = join_all(symbolication_tasks).await;

        if let Some(quota_manager) = quota_manager {
            quota_manager.finish().await;
        }

        let results: Vec<_> = symbolication_results
            .into_iter()
            .filter_map(|x| x.unwrap())
            .collect();
        let string_table = match Arc::try_unwrap(string_table) {
            Ok(string_table) => string_table.into_inner().unwrap(),
            Err(_string_table) => panic!("String table Arc still in use"),
        };

        (results, string_table)
    });

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

async fn get_lib_symbols(
    lib: wholesym::LibraryInfo,
    rvas: &[u32],
    symbol_manager: &SymbolManager,
    string_table: Arc<Mutex<StringTable>>,
) -> Option<PrecogLibrarySymbols> {
    //eprintln!("Library {} ({}) has {} rvas", lib.debug_name, lib.debug_id, rvas.len());
    let Ok(symbol_map) = symbol_manager
        .load_symbol_map(lib.debug_name.as_deref().unwrap(), lib.debug_id.unwrap())
        .await
    else {
        //eprintln!("Couldn't load symbol map for {} at {} {} ({})", lib.debug_name, lib.path, lib.debug_path, lib.debug_id);
        return None;
    };

    type FastIndexSet<V> = IndexSet<V, rustc_hash::FxBuildHasher>;
    let mut symbol_table = FastIndexSet::default();

    let mut known_addresses = Vec::new();
    for rva in rvas {
        if let Some(addr_info) = symbol_map
            .lookup(wholesym::LookupAddress::Relative(*rva))
            .await
        {
            let symbol_info =
                InternedSymbolInfo::new(&addr_info, &mut string_table.lock().unwrap());
            let index = symbol_table.insert_full(symbol_info).0;
            known_addresses.push((*rva, index));
        }
    }

    Some(PrecogLibrarySymbols {
        debug_name: lib.debug_name.unwrap(),
        debug_id: lib.debug_id.unwrap().to_string(),
        code_id: lib
            .code_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or("".to_owned()),
        symbol_table: symbol_table.into_iter().collect(),
        known_addresses,
        string_table: None,
    })
}
