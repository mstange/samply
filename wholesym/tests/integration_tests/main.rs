use std::{path::PathBuf, str::FromStr};

use debugid::DebugId;

use wholesym::CodeId;

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

#[test]
fn successful_dll() {
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
