use std::{path::PathBuf, str::FromStr};

use debugid::DebugId;

use wholesym::{CodeId, FramesLookupResult};

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
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
    assert_eq!(info.arch, None);
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
    assert_eq!(info.arch, None);
}

#[test]
fn dwz_symbolication() {
    let ls_dir = fixtures_dir().join("other").join("ls-linux");
    let ls_bin_path = ls_dir.join("ls");
    let config = wholesym::SymbolManagerConfig::default()
        .verbose(true)
        .redirect_path_for_testing(
            "/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug".into(),
            ls_dir.join("260a3e6e46db57abf718f6a3562c6eedccf269.debug"),
        )
        .redirect_path_for_testing(
            "/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug".into(),
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

    let sym = symbol_map.lookup(0xd6f4).unwrap();

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
        frames[0].file_path.as_ref().unwrap().mapped_path(),
        "./src/ls.c"
    );
    assert_eq!(
        frames[1].file_path.as_ref().unwrap().mapped_path(),
        "./src/ls.c"
    );

    // Check information coming from the supplementary file (coreutils.debug), found via absolute path in the debugaltlink:
    assert_eq!(frames[0].function.as_ref().unwrap(), "do_lstat");
    assert_eq!(frames[1].function.as_ref().unwrap(), "gobble_file");
}
