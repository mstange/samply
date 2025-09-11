use std::path::{Path, PathBuf};
use std::str::FromStr;

use debugid::DebugId;
use wholesym::{CodeId, FramesLookupResult, LookupAddress};

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

/// This is the example from the crate docs. Run it when you update the docs.
#[test]
#[ignore]
fn for_docs() {
    use wholesym::{SymbolManager, SymbolManagerConfig};

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
        if let Some(address_info) = symbol_map.lookup(LookupAddress::Relative(0xd6f4)).await {
            println!(
                "Symbol: {:#x} {}",
                address_info.symbol.address, address_info.symbol.name
            );
            if let Some(frames) = address_info.frames {
                for (i, frame) in frames.into_iter().enumerate() {
                    let function = frame.function.unwrap();

                    let file = symbol_map.resolve_source_file_path(frame.file_path.unwrap());
                    let path = file.display_path();
                    let line = frame.line_number.unwrap();
                    println!("  #{i:02} {function} at {path}:{line}");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn dwz_symbolication() {
    let ls_dir = fixtures_dir().join("other").join("ls-linux");
    let ls_bin_path = ls_dir.join("ls");
    let config = wholesym::SymbolManagerConfig::default()
        .redirect_path_for_testing(
            "/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug",
            ls_dir.join("260a3e6e46db57abf718f6a3562c6eedccf269.debug"),
        )
        .redirect_path_for_testing(
            "/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug",
            ls_dir.join("coreutils.debug"),
        );
    let symbol_manager = wholesym::SymbolManager::with_config(config);
    let symbol_map = symbol_manager
        .load_symbol_map_for_binary_at_path(&ls_bin_path, None)
        .await
        .unwrap();

    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("3E0A2663466E57DBABF718F6A3562C6E0").unwrap()
    );

    let sym = symbol_map
        .lookup_sync(LookupAddress::Relative(0xd6f4))
        .unwrap();

    let frames = match &sym.frames {
        Some(FramesLookupResult::Available(frames)) => frames,
        _ => panic!("failed to obtain debug info"),
    };

    // Check information coming from the symbol table:
    assert_eq!(&sym.symbol.name, "gobble_file.constprop.0");
    assert_eq!(sym.symbol.address, 0xd5d4);
    assert_eq!(sym.symbol.size, Some(0xebc));

    // Check information coming from the debug info found via build ID:
    assert_eq!(
        symbol_map
            .resolve_source_file_path(frames[0].file_path.unwrap())
            .raw_path(),
        "./src/ls.c"
    );
    assert_eq!(
        symbol_map
            .resolve_source_file_path(frames[1].file_path.unwrap())
            .raw_path(),
        "./src/ls.c"
    );

    // Check information coming from the supplementary file (coreutils.debug), found via absolute path in the debugaltlink:
    assert_eq!(frames[0].function.as_ref().unwrap(), "do_lstat");
    assert_eq!(frames[1].function.as_ref().unwrap(), "gobble_file");
}

mod simple_example {
    use std::pin::Pin;

    use futures::Future;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestAddressInfo<'a> {
        symbol: (&'a str, u32, u32),
        frames: &'a [(&'a str, &'a str, u32)],
    }

    async fn test_address<F: FnOnce(TestAddressInfo)>(
        symbol_map: &wholesym::SymbolMap,
        relative_address: u32,
        f: F,
    ) {
        let address_info = symbol_map
            .lookup(LookupAddress::Relative(relative_address))
            .await
            .unwrap();
        let frames = address_info.frames.unwrap();
        let test_frames: Vec<_> = frames
            .iter()
            .map(|frame| {
                (
                    frame.function.as_deref().unwrap(),
                    symbol_map
                        .resolve_source_file_path(frame.file_path.unwrap())
                        .raw_path()
                        .to_owned(),
                    frame.line_number.unwrap(),
                )
            })
            .collect();
        let test_frames: Vec<_> = test_frames
            .iter()
            .map(|(f, p, l)| (*f, p.as_str(), *l))
            .collect();
        let test_address_info = TestAddressInfo {
            symbol: (
                &address_info.symbol.name,
                address_info.symbol.address,
                address_info.symbol.size.unwrap(),
            ),
            frames: &test_frames,
        };
        f(test_address_info);
    }

    async fn run_single_test<
        F: FnOnce(&wholesym::SymbolMap) -> Pin<Box<dyn Future<Output = ()> + '_>>,
    >(
        bin_path: &Path,
        redirect_paths: &[(&str, PathBuf)],
        expected_debug_id: DebugId,
        test_fn: F,
    ) {
        let mut config = wholesym::SymbolManagerConfig::default();
        for (s, path) in redirect_paths {
            config = config.redirect_path_for_testing(s, path);
        }
        let symbol_manager = wholesym::SymbolManager::with_config(config);
        let symbol_map = symbol_manager
            .load_symbol_map_for_binary_at_path(bin_path, None)
            .await
            .unwrap();

        assert_eq!(symbol_map.debug_id(), expected_debug_id);
        test_fn(&symbol_map).await;
    }

    async fn linux_simple_example_test_fn(symbol_map: &wholesym::SymbolMap) {
        test_address(symbol_map, 0xb14, |t| {
            assert_eq!(
                t,
                TestAddressInfo {
                    symbol: ("file1_func2(int)", 0xafc, 0x2c),
                    frames: &[
                        (
                            "file1_func3(int, int)",
                            "/home/ubuntu/code/samply/fixtures/other/simple-example/src/file1.h",
                            5,
                        ),
                        (
                            "file1_func2(int)",
                            "/home/ubuntu/code/samply/fixtures/other/simple-example/src/file1.cpp",
                            13,
                        ),
                    ],
                }
            )
        })
        .await;

        test_address(symbol_map, 0xb98, |t| {
            assert_eq!(
                t,
                TestAddressInfo {
                    symbol: ("file2_func1(int)", 0xb74, 0x38),
                    frames: &[
                        (
                            "file2_func3(int, int)",
                            "/home/ubuntu/code/samply/fixtures/other/simple-example/src/file2.h",
                            5,
                        ),
                        (
                            "file2_func2(int)",
                            "/home/ubuntu/code/samply/fixtures/other/simple-example/src/file2.cpp",
                            11,
                        ),
                        (
                            "file2_func1(int)",
                            "/home/ubuntu/code/samply/fixtures/other/simple-example/src/file2.cpp",
                            7,
                        ),
                    ],
                }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn run_test_regular_debuglink() {
        let regular_debuglink_dir =
            fixtures_dir().join("other/simple-example/out/regular-debuglink");
        run_single_test(
            &regular_debuglink_dir.join("main"),
            &[],
            DebugId::from_breakpad("0C3E1D589F360C231BC06257AD3D38270").unwrap(),
            |sm| Box::pin(linux_simple_example_test_fn(sm)),
        )
        .await;
    }

    #[tokio::test]
    async fn run_test_with_dwo() {
        let dwo_obj_dir = fixtures_dir().join("other/simple-example/out/with-dwo");
        run_single_test(
            &dwo_obj_dir.join("main"),
            &[
                (
                    "/home/ubuntu/code/samply/fixtures/other/simple-example/out/with-dwo/file1.dwo",
                    dwo_obj_dir.join("file1.dwo"),
                ),
                (
                    "/home/ubuntu/code/samply/fixtures/other/simple-example/out/with-dwo/file2.dwo",
                    dwo_obj_dir.join("file2.dwo"),
                ),
            ],
            DebugId::from_breakpad("64EC1ADF3B4779940896B95FEDD58FB20").unwrap(),
            |sm| Box::pin(linux_simple_example_test_fn(sm)),
        )
        .await;
    }

    #[tokio::test]
    async fn run_test_with_dwp() {
        let dwp_obj_dir = fixtures_dir().join("other/simple-example/out/with-dwp");
        run_single_test(
            &dwp_obj_dir.join("main"),
            &[],
            DebugId::from_breakpad("AA203F622728BC24591A89512845E0900").unwrap(),
            |sm| Box::pin(linux_simple_example_test_fn(sm)),
        )
        .await;
    }

    #[tokio::test]
    async fn run_test_dwp_debuglink() {
        let dwp_debuglink_obj_dir = fixtures_dir().join("other/simple-example/out/dwp-debuglink");
        run_single_test(
            &dwp_debuglink_obj_dir.join("main"),
            &[],
            DebugId::from_breakpad("057C5D3DF90D23FDE7A32D40698479AC0").unwrap(),
            |sm| Box::pin(linux_simple_example_test_fn(sm)),
        )
        .await;
    }

    async fn mac_simple_example_test_fn(symbol_map: &wholesym::SymbolMap) {
        test_address(symbol_map, 0x3ac0, |t| {
            assert_eq!(
                t,
                TestAddressInfo {
                    symbol: ("file1_func1(int)", 0x3a7c, 0x60),
                    frames: &[
                        (
                            "file1_func2(int)",
                            "/Users/mstange/code/samply/fixtures/other/simple-example/src/file1.cpp",
                            13,
                        ),
                        (
                            "file1_func1(int)",
                            "/Users/mstange/code/samply/fixtures/other/simple-example/src/file1.cpp",
                            9,
                        ),
                    ],
                }
            )
        })
        .await;

        test_address(symbol_map, 0x3b30, |t| {
            assert_eq!(
                t,
                TestAddressInfo {
                    symbol: ("file2_func1(int)", 0x3b08, 0x3c),
                    frames: &[
                        (
                            "file2_func3(int, int)",
                            "/Users/mstange/code/samply/fixtures/other/simple-example/src/file2.h",
                            5,
                        ),
                        (
                            "file2_func2(int)",
                            "/Users/mstange/code/samply/fixtures/other/simple-example/src/file2.cpp",
                            11,
                        ),
                        (
                            "file2_func1(int)",
                            "/Users/mstange/code/samply/fixtures/other/simple-example/src/file2.cpp",
                            7,
                        ),
                    ],
                }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn run_test_mac_oso() {
        let mac_oso_dir = fixtures_dir().join("other/simple-example/out/mac-oso");
        run_single_test(
            &mac_oso_dir.join("main"),
            &[(
                "/Users/mstange/code/samply/fixtures/other/simple-example/out/mac-oso/file1.o",
                mac_oso_dir.join("file1.o"),
            ), (
                "/Users/mstange/code/samply/fixtures/other/simple-example/out/mac-oso/libfile23.a",
                mac_oso_dir.join("libfile23.a"),
            )],
            DebugId::from_breakpad("FF78692C66BD35DB9476D5060EA5BD830").unwrap(),
            |sm| Box::pin(mac_simple_example_test_fn(sm))
        )
        .await;
    }

    #[tokio::test]
    async fn run_test_mac_dsym() {
        let mac_dsym_dir = fixtures_dir().join("other/simple-example/out/mac-dsym");
        run_single_test(
            &mac_dsym_dir.join("main"),
            &[(
                "/Users/mstange/code/samply/fixtures/other/simple-example/out/mac-dsym/main.dSYM/Contents/Resources/DWARF/main",
                mac_dsym_dir.join("main.dSYM/Contents/Resources/DWARF/main"),
            )],
            DebugId::from_breakpad("CF441AF5BB7E35678D44451D69214D920").unwrap(),
            |sm| Box::pin(mac_simple_example_test_fn(sm))
        )
        .await;
    }
}
