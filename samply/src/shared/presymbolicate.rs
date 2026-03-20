use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use futures_util::future::join_all;
use fxprof_processed_profile::symbol_info::{
    AddressFrame as ProfileAddressFrame, AddressInfo as ProfileAddressInfo, LibSymbolInfo,
    ProfileSymbolInfo, SymbolStringIndex, SymbolStringTable,
};
use fxprof_processed_profile::LibraryHandle;
use rustc_hash::FxHashMap;
use wholesym::{
    FunctionNameHandle, MultiArchDisambiguator, SourceFilePathHandle, SymbolManager, SymbolMap,
    SymbolNameHandle,
};

use crate::symbols::create_symbol_manager_and_quota_manager;

use super::prop_types::SymbolProps;

struct StringTableAdapterForSymbolTable<'a> {
    symbol_map: &'a SymbolMap,
    string_table: &'a mut SymbolStringTable,
    function_name_map: FxHashMap<FunctionNameHandle, SymbolStringIndex>,
    symbol_name_map: FxHashMap<SymbolNameHandle, SymbolStringIndex>,
    source_file_path_map: FxHashMap<SourceFilePathHandle, SymbolStringIndex>,
}

impl<'a> StringTableAdapterForSymbolTable<'a> {
    pub fn map_function_name(&mut self, handle: FunctionNameHandle) -> SymbolStringIndex {
        *self.function_name_map.entry(handle).or_insert_with(|| {
            let function_name = self.symbol_map.resolve_function_name(handle);
            self.string_table.index_for_string(&function_name)
        })
    }

    pub fn map_symbol_name(&mut self, handle: SymbolNameHandle) -> SymbolStringIndex {
        *self.symbol_name_map.entry(handle).or_insert_with(|| {
            let symbol_name = self.symbol_map.resolve_symbol_name(handle);
            self.string_table.index_for_string(&symbol_name)
        })
    }

    pub fn map_source_file_path(&mut self, handle: SourceFilePathHandle) -> SymbolStringIndex {
        *self.source_file_path_map.entry(handle).or_insert_with(|| {
            let path = self.symbol_map.resolve_source_file_path(handle);
            let path_str = path
                .special_path_str()
                .unwrap_or_else(|| path.raw_path().into());
            self.string_table.index_for_string(&path_str)
        })
    }
}

fn convert_address_frame(
    frame: &wholesym::FrameDebugInfo,
    strtab: &mut StringTableAdapterForSymbolTable,
) -> Option<ProfileAddressFrame> {
    let function_handle = frame.function?;
    let function_name = strtab.map_function_name(function_handle);
    let file = frame
        .file_path
        .map(|handle| strtab.map_source_file_path(handle));

    Some(ProfileAddressFrame {
        function_name,
        file,
        line: frame.line_number,
        col: None,
        function_start_line: None,
        function_start_col: None,
    })
}

fn convert_address_info(
    info: &wholesym::AddressInfo,
    strtab: &mut StringTableAdapterForSymbolTable,
) -> ProfileAddressInfo {
    let symbol_name = strtab.map_symbol_name(info.symbol.name);
    let frames = info
        .frames
        .as_ref()
        .map(|frames| {
            frames
                .iter()
                .flat_map(|frame| convert_address_frame(frame, strtab))
                .collect()
        })
        .unwrap_or_default();
    ProfileAddressInfo {
        symbol_name,
        symbol_start_address: info.symbol.address,
        symbol_size: info.symbol.size,
        frames,
    }
}

pub fn get_presymbolicate_info(
    profile: &fxprof_processed_profile::Profile,
    symbol_props: SymbolProps,
) -> ProfileSymbolInfo {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
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
                    debug_id: if lib.debug_id.is_nil() {
                        None
                    } else {
                        Some(lib.debug_id)
                    },
                    arch: lib.arch.clone(),
                    debug_name: Some(lib.debug_name.clone()),
                    code_id: lib
                        .code_id
                        .as_ref()
                        .map(|id| wholesym::CodeId::from_str(id).expect("bad codeid")),
                };
                let rvas: Vec<u32> = rvas.into_iter().collect();
                (lib_handle, lib_info, rvas)
            })
            .collect();

        for (_lib_handle, lib_info, _rvas) in &lib_stuff {
            // Add the library to the symbol manager with all the info, so that load_symbol_map can find it later
            symbol_manager.add_known_library(lib_info.clone());
        }

        let string_table = Arc::new(Mutex::new(SymbolStringTable::new()));
        let symbol_manager = Arc::new(symbol_manager);

        let symbolication_tasks = lib_stuff.into_iter().map(|(lib_handle, lib, rvas)| {
            let symbol_manager = Arc::clone(&symbol_manager);
            let string_table = Arc::clone(&string_table);
            tokio::spawn(async move {
                get_lib_symbols(
                    lib_handle,
                    lib,
                    &rvas,
                    &symbol_manager,
                    string_table.clone(),
                )
                .await
            })
        });

        let symbolication_results = join_all(symbolication_tasks).await;

        if let Some(quota_manager) = quota_manager {
            quota_manager.finish().await;
        }

        let lib_symbols: Vec<_> = symbolication_results
            .into_iter()
            .filter_map(|x| x.unwrap())
            .collect();
        let string_table = match Arc::try_unwrap(string_table) {
            Ok(string_table) => string_table.into_inner().unwrap(),
            Err(_string_table) => panic!("String table Arc still in use"),
        };

        ProfileSymbolInfo {
            string_table,
            lib_symbols,
        }
    })
}

async fn get_lib_symbols(
    lib_handle: LibraryHandle,
    lib: wholesym::LibraryInfo,
    rvas: &[u32],
    symbol_manager: &SymbolManager,
    string_table: Arc<Mutex<SymbolStringTable>>,
) -> Option<LibSymbolInfo> {
    // eprintln!(
    //     "Library {:?} ({:?}) has {} rvas",
    //     lib.debug_name,
    //     lib.debug_id,
    //     rvas.len()
    // );
    let Ok(symbol_map) = get_lib_symbol_map(&lib, symbol_manager).await else {
        // eprintln!(
        //     "Couldn't load symbol map for {} at {} {} ({:?})",
        //     lib.debug_name.as_deref().unwrap(),
        //     lib.path.as_deref().unwrap(),
        //     lib.debug_path.as_deref().unwrap(),
        //     lib.debug_id
        // );
        return None;
    };
    get_lib_symbols_with_symbol_map(lib_handle, symbol_map, rvas, string_table).await
}

async fn get_lib_symbol_map(
    lib: &wholesym::LibraryInfo,
    symbol_manager: &SymbolManager,
) -> Result<SymbolMap, String> {
    if let (Some(debug_name), Some(debug_id)) = (lib.debug_name.as_deref(), lib.debug_id) {
        if let Ok(symbol_map) = symbol_manager.load_symbol_map(debug_name, debug_id).await {
            return Ok(symbol_map);
        }
    }
    if let Some(path) = lib.path.as_deref() {
        if let Ok(symbol_map) = symbol_manager
            .load_symbol_map_for_binary_at_path(
                Path::new(path),
                lib.arch.clone().map(MultiArchDisambiguator::Arch),
            )
            .await
        {
            return Ok(symbol_map);
        }
    }
    Err(format!(
        "Not enough information to look up symbol map for library {}",
        lib.name.as_deref().unwrap()
    ))
}

async fn get_lib_symbols_with_symbol_map(
    lib_handle: LibraryHandle,
    symbol_map: SymbolMap,
    rvas: &[u32],
    string_table: Arc<Mutex<SymbolStringTable>>,
) -> Option<LibSymbolInfo> {
    let mut sorted_addresses = Vec::new();
    let mut address_infos = Vec::new();
    for rva in rvas.iter().cloned() {
        let Some(addr_info) = symbol_map
            .lookup(wholesym::LookupAddress::Relative(rva))
            .await
        else {
            continue;
        };
        let mut string_table = string_table.lock().unwrap();

        let mut string_table = StringTableAdapterForSymbolTable {
            symbol_map: &symbol_map,
            string_table: &mut string_table,
            function_name_map: Default::default(),
            symbol_name_map: Default::default(),
            source_file_path_map: Default::default(),
        };

        let address_info = convert_address_info(&addr_info, &mut string_table);
        sorted_addresses.push(rva);
        address_infos.push(address_info);
    }

    Some(LibSymbolInfo {
        lib_handle,
        sorted_addresses,
        address_infos,
    })
}
