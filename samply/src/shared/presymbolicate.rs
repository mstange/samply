use std::str::FromStr;
use std::sync::{Arc, Mutex};

use futures_util::future::join_all;
use fxprof_processed_profile::symbol_info::{
    AddressFrame as ProfileAddressFrame, AddressInfo as ProfileAddressInfo, LibSymbolInfo,
    ProfileSymbolInfo, SymbolStringTable,
};
use fxprof_processed_profile::LibraryHandle;
use wholesym::SymbolManager;

use crate::symbols::create_symbol_manager_and_quota_manager;

use super::prop_types::SymbolProps;

fn convert_address_frame(
    frame: &wholesym::FrameDebugInfo,
    strtab: &mut SymbolStringTable,
) -> Option<ProfileAddressFrame> {
    let function_name = strtab.index_for_string(frame.function.as_ref()?);
    let file = frame.file_path.as_ref().map(|source_file_path| {
        if let Some(mapped_path) = source_file_path.special_path_str() {
            strtab.index_for_string(&mapped_path)
        } else {
            strtab.index_for_string(source_file_path.raw_path())
        }
    });

    Some(ProfileAddressFrame {
        function_name,
        file,
        line: frame.line_number,
    })
}

fn convert_address_info(
    info: &wholesym::AddressInfo,
    strtab: &mut SymbolStringTable,
) -> ProfileAddressInfo {
    let symbol_name = strtab.index_for_string(&info.symbol.name);
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
                    debug_id: Some(lib.debug_id),
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
    //eprintln!("Library {} ({}) has {} rvas", lib.debug_name, lib.debug_id, rvas.len());
    let Ok(symbol_map) = symbol_manager
        .load_symbol_map(lib.debug_name.as_deref().unwrap(), lib.debug_id.unwrap())
        .await
    else {
        //eprintln!("Couldn't load symbol map for {} at {} {} ({})", lib.debug_name, lib.path, lib.debug_path, lib.debug_id);
        return None;
    };

    let mut sorted_addresses = Vec::new();
    let mut address_infos = Vec::new();
    for rva in rvas.iter().cloned() {
        if let Some(addr_info) = symbol_map
            .lookup(wholesym::LookupAddress::Relative(rva))
            .await
        {
            let address_info = convert_address_info(&addr_info, &mut string_table.lock().unwrap());
            sorted_addresses.push(rva);
            address_infos.push(address_info);
        }
    }

    Some(LibSymbolInfo {
        lib_handle,
        sorted_addresses,
        address_infos,
    })
}
