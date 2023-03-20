use std::{path::PathBuf, str::FromStr};

use debugid::DebugId;

use wholesym::{CodeId, FramesLookupResult, LibraryInfo};

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

/// This is the example from the crate docs. Run it when you update the docs.
#[test]
#[ignore]
fn for_docs() {
    use wholesym::{FramesLookupResult, SymbolManager, SymbolManagerConfig};

    async fn run() -> Result<(), wholesym::Error> {
        // let symbol_map = symbol_manager
        //     .load_symbol_map_for_binary_at_path(std::path::Path::new("/usr/bin/ls"), None)
        //     .await?;
        // let symbol_manager = SymbolManager::with_config(SymbolManagerConfig::default());

        let ls_dir = fixtures_dir().join("other").join("ls-linux");
        let ls_bin_path = ls_dir.join("ls");
        let config = SymbolManagerConfig::default()
            .redirect_path_for_testing(
                "/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug",
                ls_dir.join("260a3e6e46db57abf718f6a3562c6eedccf269.debug"),
            )
            .redirect_path_for_testing(
                "/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug",
                ls_dir.join("coreutils.debug"),
            );
        let symbol_manager = SymbolManager::with_config(config);

        let symbol_map = symbol_manager
            .load_symbol_map_for_binary_at_path(&ls_bin_path, None)
            .await?;
        println!("Looking up 0xd6f4 in /usr/bin/ls. Results:");
        if let Some(address_info) = symbol_map.lookup_relative_address(0xd6f4) {
            println!(
                "Symbol: {:#x} {}",
                address_info.symbol.address, address_info.symbol.name
            );
            let frames = match address_info.frames {
                FramesLookupResult::Available(frames) => Some(frames),
                FramesLookupResult::External(ext_ref) => {
                    symbol_manager
                        .lookup_external(&symbol_map.symbol_file_origin(), &ext_ref)
                        .await
                }
                FramesLookupResult::Unavailable => None,
            };
            if let Some(frames) = frames {
                for (i, frame) in frames.into_iter().enumerate() {
                    let function = frame.function.unwrap();
                    let file = frame.file_path.unwrap().display_path();
                    let line = frame.line_number.unwrap();
                    println!("  #{i:02} {function} at {file}:{line}");
                }
            }
        } else {
            println!("No symbol for 0xd6f4 was found.");
        }
        Ok(())
    }
    let _ = futures::executor::block_on(run());
}

#[test]
fn dll() {
    // Compute the LibraryInfo for mozglue.dll.
    let dll_path = fixtures_dir().join("win64-ci").join("mozglue.dll");
    let info = futures::executor::block_on(
        wholesym::SymbolManager::library_info_for_binary_at_path(&dll_path, None),
    )
    .unwrap();

    assert_eq!(info.name, Some("mozglue.dll".into()));
    assert_eq!(info.code_id, CodeId::from_str("5eba814695000").ok());
    assert_eq!(info.debug_name, Some("mozglue.pdb".into()));
    assert_eq!(
        info.debug_id,
        Some(DebugId::from_breakpad("63C609072D3499F64C4C44205044422E1").unwrap())
    );
    assert_eq!(
        info.debug_path,
        Some("/builds/worker/workspace/obj-build/mozglue/build/mozglue.pdb".into())
    );
    assert_eq!(info.arch.as_deref(), Some("x86_64"));
}

#[test]
fn exe() {
    // Compute the LibraryInfo for firefox.exe.
    let exe_path = fixtures_dir().join("win64-local").join("firefox.exe");
    let info = futures::executor::block_on(
        wholesym::SymbolManager::library_info_for_binary_at_path(&exe_path, None),
    )
    .unwrap();

    assert_eq!(info.name, Some("firefox.exe".into()));
    assert_eq!(
        info.code_id,
        Some(CodeId::from_str("5EBAD356a1000").unwrap())
    );
    assert_eq!(info.debug_name, Some("firefox.pdb".into()));
    assert_eq!(
        info.debug_id,
        Some(DebugId::from_breakpad("8A913DE821D9DE764C4C44205044422E1").unwrap())
    );
    assert_eq!(
        info.debug_path,
        Some("c:\\mozilla-source\\obj-m-opt\\browser\\app\\firefox.pdb".into())
    );
    assert_eq!(info.arch.as_deref(), Some("x86_64"));
}

#[test]
fn dwz_symbolication() {
    let ls_dir = fixtures_dir().join("other").join("ls-linux");
    let ls_bin_path = ls_dir.join("ls");
    let config = wholesym::SymbolManagerConfig::default()
        .verbose(true)
        .redirect_path_for_testing(
            "/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug",
            ls_dir.join("260a3e6e46db57abf718f6a3562c6eedccf269.debug"),
        )
        .redirect_path_for_testing(
            "/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug",
            ls_dir.join("coreutils.debug"),
        );
    let symbol_manager = wholesym::SymbolManager::with_config(config);
    let symbol_map = futures::executor::block_on(
        symbol_manager.load_symbol_map_for_binary_at_path(&ls_bin_path, None),
    )
    .unwrap();

    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("3E0A2663466E57DBABF718F6A3562C6E0").unwrap()
    );

    let sym = symbol_map.lookup_relative_address(0xd6f4).unwrap();

    let frames = match &sym.frames {
        FramesLookupResult::Available(frames) => frames,
        _ => panic!("failed to obtain debug info"),
    };

    // Check information coming from the symbol table:
    assert_eq!(&sym.symbol.name, "gobble_file.constprop.0");
    assert_eq!(sym.symbol.address, 0xd5d4);
    assert_eq!(sym.symbol.size, Some(0xebc));

    // Check information coming from the debug info found via build ID:
    assert_eq!(
        frames[0].file_path.as_ref().unwrap().raw_path(),
        "./src/ls.c"
    );
    assert_eq!(
        frames[1].file_path.as_ref().unwrap().raw_path(),
        "./src/ls.c"
    );

    // Check information coming from the supplementary file (coreutils.debug), found via absolute path in the debugaltlink:
    assert_eq!(frames[0].function.as_ref().unwrap(), "do_lstat");
    assert_eq!(frames[1].function.as_ref().unwrap(), "gobble_file");
}

// This test only works on macOS 13.0.1.
#[ignore]
#[test]
fn disassemble_libcrypto() {
    let lib_info = LibraryInfo {
        debug_name: Some("libcorecrypto.dylib".into()),
        debug_id: DebugId::from_breakpad("6A5FFEB0E606324EB687DA95C362CE050").ok(),
        path: Some("/usr/lib/system/libcorecrypto.dylib".into()),
        arch: Some("arm64e".into()),
        ..Default::default()
    };

    let mut symbol_manager = wholesym::SymbolManager::with_config(Default::default());
    symbol_manager.add_known_library(lib_info);
    let response_json = futures::executor::block_on(
        symbol_manager.query_json_api("/asm/v1", r#"{"debugName":"libcorecrypto.dylib","debugId":"6A5FFEB0E606324EB687DA95C362CE050","name":"libcorecrypto.dylib","codeId":null,"startAddress":"0x5844","size":"0x1c"}"#),
    );
    assert_eq!(
        response_json,
        r#"{"startAddress":"0x5844","size":"0x1c","instructions":[[0,"hint #0x1b"],[4,"stp x29, x30, [sp, #-0x10]!"],[8,"mov x29, sp"],[12,"adrp x0, $+0x593f3000"],[16,"add x0, x0, #0x340"],[20,"ldr x8, [x0]"],[24,"blraaz x8"]]}"#
    );
}
