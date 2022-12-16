use samply_symbols::debugid::DebugId;
use samply_symbols::{
    self, CandidatePathInfo, CompactSymbolTable, Error, FileAndPathHelper, FileAndPathHelperResult,
    FileLocation, LibraryInfo, OptionallySendFuture, SymbolManager,
};
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub async fn get_table(
    debug_name: &str,
    debug_id: Option<DebugId>,
    symbol_directory: PathBuf,
) -> anyhow::Result<CompactSymbolTable> {
    let helper = Helper { symbol_directory };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let table = get_symbols_retry_id(debug_name, debug_id, &symbol_manager).await?;
    Ok(table)
}

async fn get_symbols_retry_id(
    debug_name: &str,
    debug_id: Option<DebugId>,
    symbol_manager: &SymbolManager<'_, Helper>,
) -> anyhow::Result<CompactSymbolTable> {
    let debug_id = match debug_id {
        Some(debug_id) => debug_id,
        None => {
            // No debug ID was specified. load_compact_symbol_table always wants one, so we call it twice:
            // First, with a bogus debug ID (DebugId::nil()), and then again with the debug ID that
            // it expected.
            let result = symbol_manager
                .load_compact_symbol_table(debug_name, DebugId::nil())
                .await;
            match result {
                Ok(table) => return Ok(table),
                Err(err) => match err {
                    Error::UnmatchedDebugId(expected, supplied) if supplied == DebugId::nil() => {
                        eprintln!("Using debug ID: {}", expected.breakpad());
                        expected
                    }
                    err => return Err(err.into()),
                },
            }
        }
    };
    Ok(symbol_manager
        .load_compact_symbol_table(debug_name, debug_id)
        .await?)
}

pub fn dump_table(w: &mut impl Write, table: CompactSymbolTable, full: bool) -> anyhow::Result<()> {
    let mut w = BufWriter::new(w);
    writeln!(w, "Found {} symbols.", table.addr.len())?;
    for (i, address) in table.addr.iter().enumerate() {
        if i >= 15 && !full {
            writeln!(
                w,
                "and {} more symbols. Pass --full to print the full list.",
                table.addr.len() - i
            )?;
            break;
        }

        let start_pos = table.index[i];
        let end_pos = table.index[i + 1];
        let symbol_bytes = &table.buffer[start_pos as usize..end_pos as usize];
        let symbol_string = std::str::from_utf8(symbol_bytes)?;
        writeln!(w, "{:x} {}", address, symbol_string)?;
    }
    Ok(())
}

struct Helper {
    symbol_directory: PathBuf,
}

type FileContentsType = memmap2::Mmap;

fn mmap_to_file_contents(m: memmap2::Mmap) -> FileContentsType {
    m
}

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContentsType;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let debug_name = match library_info.debug_name.as_deref() {
            Some(debug_name) => debug_name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Also consider .so.dbg files in the symbol directory.
        if debug_name.ends_with(".so") {
            let debug_debug_name = format!("{}.dbg", debug_name);
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                self.symbol_directory.join(debug_debug_name),
            )));
        }

        // And dSYM packages.
        if !debug_name.ends_with(".pdb") {
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                self.symbol_directory
                    .join(format!("{}.dSYM", debug_name))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            )));
        }

        // Finally, the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            self.symbol_directory.join(debug_name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(debug_name).to_str() {
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_arm64e")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64h")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
            }
        }

        Ok(paths)
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        async fn open_file_impl(path: PathBuf) -> FileAndPathHelperResult<FileContentsType> {
            eprintln!("Opening file {:?}", &path);
            let file = File::open(&path)?;
            let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
            Ok(mmap_to_file_contents(mmap))
        }

        let path = match location {
            FileLocation::Path(path) => path.clone(),
            FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
        };
        Box::pin(open_file_impl(path))
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let name = match library_info.name.as_deref() {
            Some(name) => name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Start with the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            self.symbol_directory.join(name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(name).to_str() {
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_arm64e")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64h")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
            }
        }

        Ok(paths)
    }
}

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

#[test]
fn successful_pdb() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox.pdb",
        DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").ok(),
        fixtures_dir().join("win64-ci"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 1321);
    assert_eq!(result.addr[776], 0x31fc0);
    assert_eq!(
            std::str::from_utf8(
                &result.buffer[result.index[776] as usize..result.index[777] as usize]
            ),
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors(sandbox::IPCInfo*, sandbox::CountedBuffer*)")
        );
}

#[test]
fn successful_pdb2() {
    let result = futures::executor::block_on(crate::get_table(
        "mozglue.pdb",
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
        fixtures_dir().join("win64-local"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 1080);
    assert_eq!(result.addr[454], 0x34670);
    assert_eq!(
        std::str::from_utf8(&result.buffer[result.index[454] as usize..result.index[455] as usize]),
        Ok("mozilla::baseprofiler::profiler_get_profile(double, bool, bool)")
    );
}

#[test]
fn successful_dll() {
    // The breakpad ID, symbol address and symbol name in this test are the same as
    // in the previous PDB test. The difference is that this test is looking at the
    // exports in the DLL rather than the symbols in the PDB.
    let result = futures::executor::block_on(crate::get_table(
        "mozglue.dll",
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
        fixtures_dir().join("win64-local"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 947);

    // Test an export symbol.
    assert_eq!(result.addr[430], 0x34670);
    assert_eq!(
            std::str::from_utf8(
                &result.buffer[result.index[430] as usize..result.index[431] as usize]
            ),
            Ok("?profiler_get_profile@baseprofiler@mozilla@@YA?AV?$UniquePtr@$$BY0A@DV?$DefaultDelete@$$BY0A@D@mozilla@@@2@N_N0@Z")
        );

    // Test a placeholder symbol.
    assert_eq!(result.addr[765], 0x56420);
    assert_eq!(
        std::str::from_utf8(&result.buffer[result.index[765] as usize..result.index[766] as usize]),
        Ok("fun_56420")
    );
}

#[test]
fn successful_pdb_unspecified_id() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox.pdb",
        None,
        fixtures_dir().join("win64-ci"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 1321);
    assert_eq!(result.addr[776], 0x31fc0);
    assert_eq!(
            std::str::from_utf8(
                &result.buffer[result.index[776] as usize..result.index[777] as usize]
            ),
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors(sandbox::IPCInfo*, sandbox::CountedBuffer*)")
        );
}

#[test]
fn unsuccessful_pdb_wrong_id() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox.pdb",
        DebugId::from_breakpad("AA152DEBFFFFFFFFFFFFFFFFF044422E1").ok(),
        fixtures_dir().join("win64-ci"),
    ));
    assert!(result.is_err());
    let err = match result {
        Ok(_) => panic!("Shouldn't have succeeded with wrong breakpad ID"),
        Err(err) => err,
    };
    let err = match err.downcast::<Error>() {
        Ok(err) => err,
        Err(_) => panic!("wrong error type"),
    };
    match err {
        Error::UnmatchedDebugId(expected, actual) => {
            assert_eq!(
                expected.breakpad().to_string(),
                "AA152DEB2D9B76084C4C44205044422E1"
            );
            assert_eq!(
                actual.breakpad().to_string(),
                "AA152DEBFFFFFFFFFFFFFFFFF044422E1"
            );
        }
        _ => panic!("wrong Error subtype"),
    }
}

#[test]
fn unspecified_id_fat_arch() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox",
        None,
        fixtures_dir().join("macos-ci"),
    ));
    assert!(result.is_err());
    let err = match result {
        Ok(_) => panic!("Shouldn't have succeeded with unspecified breakpad ID"),
        Err(err) => err,
    };
    let err = match err.downcast::<Error>() {
        Ok(err) => err,
        Err(_) => panic!("wrong error type"),
    };
    match err {
        Error::NoMatchMultiArch(members) => {
            let member_ids: Vec<DebugId> = members
                .iter()
                .filter_map(|(_, _, _, debug_id)| *debug_id)
                .collect();
            assert_eq!(member_ids.len(), 2);
            assert!(member_ids
                .contains(&DebugId::from_breakpad("B993FABD8143361AB199F7DE9DF7E4360").unwrap()));
            assert!(member_ids
                .contains(&DebugId::from_breakpad("8E7B0ED0B04F3FCCA05E139E5250BA720").unwrap()));
        }
        _ => panic!("wrong Error subtype: {:?}", err),
    }
}

#[test]
fn fat_arch_1() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox",
        DebugId::from_breakpad("B993FABD8143361AB199F7DE9DF7E4360").ok(),
        fixtures_dir().join("macos-ci"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 13);
    assert_eq!(result.addr[9], 0x2730);
    assert_eq!(
        std::str::from_utf8(&result.buffer[result.index[9] as usize..result.index[10] as usize]),
        Ok("__ZN7mozilla20ProfileChunkedBuffer17ResetChunkManagerEv")
    );
}

#[test]
fn fat_arch_2() {
    let result = futures::executor::block_on(crate::get_table(
        "firefox",
        DebugId::from_breakpad("8E7B0ED0B04F3FCCA05E139E5250BA720").ok(),
        fixtures_dir().join("macos-ci"),
    ));
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.addr.len(), 13);
    assert_eq!(result.addr[9], 0x759c);
    assert_eq!(
        std::str::from_utf8(&result.buffer[result.index[9] as usize..result.index[10] as usize]),
        Ok("__ZN7mozilla20ProfileChunkedBuffer17ResetChunkManagerEv")
    );
}

#[test]
fn linux_nonzero_base_address() {
    let helper = Helper {
        symbol_directory: fixtures_dir().join("linux64-ci"),
    };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let symbol_map =
        futures::executor::block_on(symbol_manager.load_symbol_map_for_binary_at_path(
            &fixtures_dir().join("linux64-ci").join("firefox"),
            None,
        ))
        .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("83CA53B0E8272691CEFCD79178D33D5C0").unwrap()
    );
    assert_eq!(symbol_map.lookup(0x1700), None);
    assert_eq!(symbol_map.lookup(0x18a0).unwrap().symbol.name, "start");
    assert_eq!(symbol_map.lookup(0x19ea).unwrap().symbol.name, "main");
    assert_eq!(
        symbol_map.lookup(0x1a60).unwrap().symbol.name,
        "_libc_csu_init"
    );
}

#[test]
fn example_linux() {
    let helper = Helper {
        symbol_directory: fixtures_dir().join("other"),
    };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let symbol_map =
        futures::executor::block_on(symbol_manager.load_symbol_map_for_binary_at_path(
            &fixtures_dir().join("other").join("example-linux"),
            None,
        ))
        .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("BE4E976C325246EE9D6B7847A670B2A90").unwrap()
    );
    assert_eq!(&symbol_map.lookup(0x1156).unwrap().symbol.name, "main");
    assert_eq!(symbol_map.lookup(0x1158), None, "Gap between main and f");
    assert_eq!(&symbol_map.lookup(0x1160).unwrap().symbol.name, "f");
}

#[test]
fn example_linux_fallback() {
    let helper = Helper {
        symbol_directory: fixtures_dir().join("other"),
    };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let symbol_map =
        futures::executor::block_on(symbol_manager.load_symbol_map_for_binary_at_path(
            &fixtures_dir().join("other").join("example-linux-fallback"),
            None,
        ))
        .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("8BA453B7838CDDC62F00004885C074020").unwrap()
    );
    // no _stack_chk_fail@@GLIBC_2.4 please
    assert_eq!(symbol_map.lookup(0x6), None);
}

#[test]
fn compare_snapshot() {
    let table = futures::executor::block_on(crate::get_table(
        "mozglue.pdb",
        DebugId::from_breakpad("63C609072D3499F64C4C44205044422E1").ok(),
        fixtures_dir().join("win64-ci"),
    ))
    .unwrap();
    let mut output: Vec<u8> = Vec::new();
    crate::dump_table(&mut output, table, true).unwrap();

    let mut snapshot_file = File::open(
        fixtures_dir()
            .join("snapshots")
            .join("win64-ci-mozglue.pdb.txt"),
    )
    .unwrap();
    let mut expected: Vec<u8> = Vec::new();
    snapshot_file.read_to_end(&mut expected).unwrap();

    if output != expected {
        let mut output_file = File::create(
            fixtures_dir()
                .join("snapshots")
                .join("win64-ci-mozglue.pdb.txt.snap"),
        )
        .unwrap();
        output_file.write_all(&output).unwrap();
    }

    let output = std::str::from_utf8(&output).unwrap();
    let expected = std::str::from_utf8(&expected).unwrap();

    assert_eq!(output, expected);
}
