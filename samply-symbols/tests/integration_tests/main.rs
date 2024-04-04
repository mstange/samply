use samply_symbols::debugid::DebugId;
use samply_symbols::{
    self, CandidatePathInfo, CompactSymbolTable, Error, FileAndPathHelper, FileAndPathHelperResult,
    FileLocation, LibraryInfo, MultiArchDisambiguator, OptionallySendFuture, SymbolManager,
    SymbolMap,
};
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

async fn get_symbol_map_with_dyld_cache_fallback(
    symbol_manager: &SymbolManager<Helper>,
    path: &Path,
    debug_id: Option<DebugId>,
) -> Result<SymbolMap<Helper>, Error> {
    let might_be_in_dyld_shared_cache = path.starts_with("/usr/") || path.starts_with("/System/");

    let disambiguator = debug_id.map(MultiArchDisambiguator::DebugId);
    let location = FileLocationType(path.to_owned());

    match symbol_manager
        .load_symbol_map_from_location(location, disambiguator.clone())
        .await
    {
        Ok(symbol_map) => Ok(symbol_map),
        Err(Error::HelperErrorDuringOpenFile(_, _)) if might_be_in_dyld_shared_cache => {
            // The file at the given path could not be opened, so it probably doesn't exist.
            // Check the dyld cache.
            symbol_manager
                .load_symbol_map_for_dyld_cache_image(&path.to_string_lossy(), disambiguator)
                .await
        }
        Err(e) => Err(e),
    }
}

pub async fn get_table(
    symbol_file_path: &Path,
    debug_id: Option<DebugId>,
) -> anyhow::Result<CompactSymbolTable> {
    let helper = Helper {
        symbol_directory: symbol_file_path.parent().unwrap().to_path_buf(),
    };
    let symbol_manager = SymbolManager::with_helper(helper);
    let symbol_map =
        get_symbol_map_with_dyld_cache_fallback(&symbol_manager, symbol_file_path, debug_id)
            .await?;
    if let Some(expected_debug_id) = debug_id {
        if symbol_map.debug_id() != expected_debug_id {
            return Err(Error::UnmatchedDebugId(symbol_map.debug_id(), expected_debug_id).into());
        }
    }
    Ok(CompactSymbolTable::from_symbol_map(&symbol_map))
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
        writeln!(w, "{address:x} {symbol_string}")?;
    }
    Ok(())
}

struct Helper {
    symbol_directory: PathBuf,
}

type FileContentsType = memmap2::Mmap;

#[derive(Clone)]
struct FileLocationType(PathBuf);

impl FileLocationType {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

impl std::fmt::Display for FileLocationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.to_string_lossy().fmt(f)
    }
}

impl FileLocation for FileLocationType {
    fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self> {
        let mut filename = self.0.file_name().unwrap().to_owned();
        filename.push(suffix);
        Some(Self(self.0.with_file_name(filename)))
    }

    fn location_for_external_object_file(&self, object_file: &str) -> Option<Self> {
        Some(Self(object_file.into()))
    }

    fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self> {
        Some(Self(pdb_path_in_binary.into()))
    }

    fn location_for_source_file(&self, source_file_path: &str) -> Option<Self> {
        Some(Self(source_file_path.into()))
    }

    fn location_for_breakpad_symindex(&self) -> Option<Self> {
        Some(Self(self.0.with_extension("symindex")))
    }

    fn location_for_dwo(&self, _comp_dir: &str, _path: &str) -> Option<Self> {
        None // TODO
    }

    fn location_for_dwp(&self) -> Option<Self> {
        let mut s = self.0.as_os_str().to_os_string();
        s.push(".dwp");
        Some(Self(s.into()))
    }
}

fn mmap_to_file_contents(m: memmap2::Mmap) -> FileContentsType {
    m
}

impl FileAndPathHelper for Helper {
    type F = FileContentsType;
    type FL = FileLocationType;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<Self::FL>>> {
        let debug_name = match library_info.debug_name.as_deref() {
            Some(debug_name) => debug_name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Also consider .so.dbg files in the symbol directory.
        if debug_name.ends_with(".so") {
            let debug_debug_name = format!("{debug_name}.dbg");
            paths.push(CandidatePathInfo::SingleFile(FileLocationType(
                self.symbol_directory.join(debug_debug_name),
            )));
        }

        // And dSYM packages.
        if !debug_name.ends_with(".pdb") {
            paths.push(CandidatePathInfo::SingleFile(FileLocationType(
                self.symbol_directory
                    .join(format!("{debug_name}.dSYM"))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            )));
        }

        // Finally, the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocationType(
            self.symbol_directory.join(debug_name),
        )));

        Ok(paths)
    }

    fn load_file(
        &self,
        location: Self::FL,
    ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + '_>>
    {
        Box::pin(async {
            let path = location.0;
            eprintln!("Opening file {:?}", &path);
            let file = File::open(&path)?;
            let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
            Ok(mmap_to_file_contents(mmap))
        })
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<Self::FL>>> {
        let name = match library_info.name.as_deref() {
            Some(name) => name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Start with the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocationType(
            self.symbol_directory.join(name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(name).to_str() {
                paths.extend(
                    self.get_dyld_shared_cache_paths(None)
                        .unwrap()
                        .into_iter()
                        .map(|dyld_cache_path| CandidatePathInfo::InDyldCache {
                            dyld_cache_path,
                            dylib_path: dylib_path.to_string(),
                        }),
                );
            }
        }

        Ok(paths)
    }

    fn get_dyld_shared_cache_paths(
        &self,
        _arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<FileLocationType>> {
        Ok(vec![
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_arm64e"),
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64h"),
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64"),
        ])
    }
}

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

#[test]
fn successful_pdb() {
    let result = futures::executor::block_on(crate::get_table(
        &fixtures_dir().join("win64-ci").join("firefox.pdb"),
        DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").ok(),
    ))
    .unwrap();
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
        &fixtures_dir().join("win64-local").join("mozglue.pdb"),
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
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
        &fixtures_dir().join("win64-local").join("mozglue.dll"),
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
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
        &fixtures_dir().join("win64-ci").join("firefox.pdb"),
        None,
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
        &fixtures_dir().join("win64-ci").join("firefox.pdb"),
        DebugId::from_breakpad("AA152DEBFFFFFFFFFFFFFFFFF044422E1").ok(),
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
        &fixtures_dir().join("macos-ci").join("firefox"),
        None,
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
        Error::NoDisambiguatorForFatArchive(members) => {
            let member_ids: Vec<DebugId> = members
                .iter()
                .filter_map(|m| m.uuid)
                .map(DebugId::from_uuid)
                .collect();
            assert_eq!(member_ids.len(), 2);
            assert!(member_ids
                .contains(&DebugId::from_breakpad("B993FABD8143361AB199F7DE9DF7E4360").unwrap()));
            assert!(member_ids
                .contains(&DebugId::from_breakpad("8E7B0ED0B04F3FCCA05E139E5250BA720").unwrap()));
        }
        _ => panic!("wrong Error subtype: {err:?}"),
    }
}

#[test]
fn fat_arch_1() {
    let result = futures::executor::block_on(crate::get_table(
        &fixtures_dir().join("macos-ci").join("firefox"),
        DebugId::from_breakpad("B993FABD8143361AB199F7DE9DF7E4360").ok(),
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
        &fixtures_dir().join("macos-ci").join("firefox"),
        DebugId::from_breakpad("8E7B0ED0B04F3FCCA05E139E5250BA720").ok(),
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
    let symbol_manager = SymbolManager::with_helper(helper);
    let symbol_map = futures::executor::block_on(symbol_manager.load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("linux64-ci").join("firefox")),
        None,
    ))
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("83CA53B0E8272691CEFCD79178D33D5C0").unwrap()
    );
    assert_eq!(symbol_map.lookup_relative_address(0x1700), None);
    assert_eq!(
        symbol_map
            .lookup_relative_address(0x18a0)
            .unwrap()
            .symbol
            .name,
        "start"
    );
    assert_eq!(
        symbol_map
            .lookup_relative_address(0x19ea)
            .unwrap()
            .symbol
            .name,
        "main"
    );
    assert_eq!(
        symbol_map
            .lookup_relative_address(0x1a60)
            .unwrap()
            .symbol
            .name,
        "_libc_csu_init"
    );

    // Compare relative addresses, SVMAs, and file offsets.
    // For this firefox binary, the segment information is as follows (from `llvm-readelf --segments`):
    // Type           Offset   VirtAddr           PhysAddr           FileSiz  MemSiz   Flg Align
    // LOAD           0x000000 0x0000000000200000 0x0000000000200000 0x000894 0x000894 R   0x1000
    // LOAD           0x0008a0 0x00000000002018a0 0x00000000002018a0 0x0002f0 0x0002f0 R E 0x1000
    // LOAD           0x000b90 0x0000000000202b90 0x0000000000202b90 0x000200 0x000200 RW  0x1000
    // LOAD           0x000d90 0x0000000000203d90 0x0000000000203d90 0x000068 0x000069 RW  0x1000
    //
    // [Nr] Name              Type            Address          Off    Size   ES Flg Lk Inf Al
    // ...
    // [15] .text             PROGBITS        00000000002018a0 0008a0 000232 00  AX  0   0 16

    assert_eq!(
        symbol_map.lookup_offset(0x8a0).unwrap(),
        symbol_map.lookup_relative_address(0x18a0).unwrap(),
    );
    assert_eq!(
        symbol_map.lookup_relative_address(0x18a0).unwrap(),
        symbol_map.lookup_svma(0x2018a0).unwrap(),
    );
}

#[test]
fn example_linux() {
    let helper = Helper {
        symbol_directory: fixtures_dir().join("other"),
    };
    let symbol_manager = SymbolManager::with_helper(helper);
    let symbol_map = futures::executor::block_on(symbol_manager.load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("other").join("example-linux")),
        None,
    ))
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("BE4E976C325246EE9D6B7847A670B2A90").unwrap()
    );
    assert_eq!(
        &symbol_map
            .lookup_relative_address(0x1156)
            .unwrap()
            .symbol
            .name,
        "main"
    );
    assert_eq!(
        symbol_map.lookup_relative_address(0x1158),
        None,
        "Gap between main and f"
    );
    assert_eq!(
        &symbol_map
            .lookup_relative_address(0x1160)
            .unwrap()
            .symbol
            .name,
        "f"
    );
}

#[test]
fn example_linux_fallback() {
    let helper = Helper {
        symbol_directory: fixtures_dir().join("other"),
    };
    let symbol_manager = SymbolManager::with_helper(helper);
    let symbol_map = futures::executor::block_on(symbol_manager.load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("other").join("example-linux-fallback")),
        None,
    ))
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("C3FC2519F439E42A970B693775586AA80").unwrap()
    );
    // no _stack_chk_fail@@GLIBC_2.4 please
    assert_eq!(symbol_map.lookup_relative_address(0x6), None);
}

#[test]
fn compare_snapshot() {
    let table = futures::executor::block_on(crate::get_table(
        &fixtures_dir().join("win64-ci").join("mozglue.pdb"),
        DebugId::from_breakpad("63C609072D3499F64C4C44205044422E1").ok(),
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
    // Strip \r which git sometimes automatically inserts on Windows
    expected.retain(|x| *x != b'\r');

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
