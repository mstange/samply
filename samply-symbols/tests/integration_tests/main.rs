use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use object::macho::{DyldSubCacheEntryV1, DyldSubCacheEntryV2};
use object::Endianness;
use samply_symbols::debugid::DebugId;
use samply_symbols::{
    CompactSymbolTable, DyldCacheLoad, Error, FileLoadResult, FileLocation, FileTypes, LoadStep,
    LoadSymbolMap, LookupAddress, MultiArchDisambiguator, NeedsFiles, SymbolMap, SymbolMapLoadStep,
};

fn drive_symbol_map_load(sm: &mut LoadSymbolMap<TestFileTypes>) {
    loop {
        match sm.poll() {
            SymbolMapLoadStep::NeedFile { location, .. } => {
                let location = location.clone();
                let result = load_file(location);
                sm.provide(result);
            }
            SymbolMapLoadStep::NeedDebugLinkCandidates { .. } => {
                sm.provide_candidates(Vec::new());
            }
            SymbolMapLoadStep::NeedSupplementaryCandidates { .. } => {
                sm.provide_candidates(Vec::new());
            }
            SymbolMapLoadStep::Done => return,
        }
    }
}

/// Drive a [`LoadSymbolMap`] state machine to completion.
fn load_symbol_map_from_location(
    location: FileLocationType,
    disambiguator: Option<MultiArchDisambiguator>,
) -> Result<SymbolMap<TestFileTypes>, Error> {
    let mut sm = LoadSymbolMap::<TestFileTypes>::new(location, disambiguator);
    drive_symbol_map_load(&mut sm);
    sm.finish()
}

fn get_symbol_map_with_dyld_cache_fallback(
    path: &Path,
    debug_id: Option<DebugId>,
) -> Result<SymbolMap<TestFileTypes>, Error> {
    let might_be_in_dyld_shared_cache = path.starts_with("/usr/") || path.starts_with("/System/");

    let disambiguator = debug_id.map(MultiArchDisambiguator::DebugId);
    let location = FileLocationType(path.to_owned());

    let mut sm = LoadSymbolMap::<TestFileTypes>::new(location, disambiguator.clone());
    drive_symbol_map_load(&mut sm);
    match sm.finish() {
        Ok(symbol_map) => Ok(symbol_map),
        Err(Error::OpenFile(_, _)) if might_be_in_dyld_shared_cache => {
            // Fall back to enumerating dyld shared cache locations.
            let dylib_path = path.to_string_lossy().into_owned();
            let dyld_cache_paths = vec![
                FileLocationType::new("/System/Library/dyld/dyld_shared_cache_arm64e"),
                FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64h"),
                FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64"),
            ];
            let expected_debug_id = match &disambiguator {
                Some(MultiArchDisambiguator::DebugId(id)) => Some(*id),
                _ => None,
            };
            let mut last_err: Option<Error> = None;
            for dyld_cache_path in dyld_cache_paths {
                let mut sm = LoadSymbolMap::<TestFileTypes>::for_dyld_cache(
                    dyld_cache_path,
                    dylib_path.clone(),
                );
                drive_symbol_map_load(&mut sm);
                match sm.finish() {
                    Ok(symbol_map) => {
                        if let Some(expected) = expected_debug_id {
                            if symbol_map.debug_id() == expected {
                                return Ok(symbol_map);
                            }
                            last_err =
                                Some(Error::UnmatchedDebugId(symbol_map.debug_id(), expected));
                        } else {
                            return Ok(symbol_map);
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            Err(last_err.unwrap_or(Error::NoCandidatePathForDyldCache))
        }
        Err(e) => Err(e),
    }
}

pub fn get_table(
    symbol_file_path: &Path,
    debug_id: Option<DebugId>,
) -> anyhow::Result<CompactSymbolTable> {
    let symbol_map = get_symbol_map_with_dyld_cache_fallback(symbol_file_path, debug_id)?;
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

struct TestFileTypes;

fn load_file(location: FileLocationType) -> FileLoadResult<FileContentsType> {
    let path = location.0;
    eprintln!("Opening file {:?}", &path);
    let file = File::open(&path)?;
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
    Ok(mmap_to_file_contents(mmap))
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

impl FileTypes for TestFileTypes {
    type F = FileContentsType;
    type FL = FileLocationType;
}

fn fixtures_dir() -> PathBuf {
    let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    this_dir.join("..").join("fixtures")
}

#[test]
fn successful_pdb() {
    let result = crate::get_table(
        &fixtures_dir().join("win64-ci").join("firefox.pdb"),
        DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").ok(),
    )
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
    let result = crate::get_table(
        &fixtures_dir().join("win64-local").join("mozglue.pdb"),
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
    );
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
    let result = crate::get_table(
        &fixtures_dir().join("win64-local").join("mozglue.dll"),
        DebugId::from_breakpad("B3CC644ECC086E044C4C44205044422E1").ok(),
    );
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
    let result = crate::get_table(&fixtures_dir().join("win64-ci").join("firefox.pdb"), None);
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
    let result = crate::get_table(
        &fixtures_dir().join("win64-ci").join("firefox.pdb"),
        DebugId::from_breakpad("AA152DEBFFFFFFFFFFFFFFFFF044422E1").ok(),
    );
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
    let result = crate::get_table(&fixtures_dir().join("macos-ci").join("firefox"), None);
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
    let result = crate::get_table(
        &fixtures_dir().join("macos-ci").join("firefox"),
        DebugId::from_breakpad("B993FABD8143361AB199F7DE9DF7E4360").ok(),
    );
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
    let result = crate::get_table(
        &fixtures_dir().join("macos-ci").join("firefox"),
        DebugId::from_breakpad("8E7B0ED0B04F3FCCA05E139E5250BA720").ok(),
    );
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
    let symbol_map = load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("linux64-ci").join("firefox")),
        None,
    )
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("83CA53B0E8272691CEFCD79178D33D5C0").unwrap()
    );
    assert_eq!(
        symbol_map.lookup_sync(LookupAddress::Relative(0x1700)),
        None
    );
    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x18a0))
        .unwrap();
    assert_eq!(symbol_map.resolve_symbol_name(result.symbol.name), "start");

    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x19ea))
        .unwrap();
    assert_eq!(symbol_map.resolve_symbol_name(result.symbol.name), "main");

    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x1a60))
        .unwrap();
    assert_eq!(
        symbol_map.resolve_symbol_name(result.symbol.name),
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
        symbol_map
            .lookup_sync(LookupAddress::FileOffset(0x8a0))
            .unwrap(),
        symbol_map
            .lookup_sync(LookupAddress::Relative(0x18a0))
            .unwrap(),
    );
    assert_eq!(
        symbol_map
            .lookup_sync(LookupAddress::Relative(0x18a0))
            .unwrap(),
        symbol_map
            .lookup_sync(LookupAddress::Svma(0x2018a0))
            .unwrap(),
    );
}

#[test]
fn nonpie_elf_fde_symbols_do_not_shadow_real_symbols() {
    let fixture_path = fixtures_dir()
        .join("other")
        .join("issue-776-nonpie-aarch64");
    let symbol_map = load_symbol_map_from_location(FileLocationType(fixture_path), None).unwrap();

    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x4580))
        .unwrap();
    assert_eq!(
        symbol_map.resolve_symbol_name(result.symbol.name),
        "func_74"
    );
    assert_eq!(result.symbol.address, 0x44f0);
}

#[test]
fn example_linux() {
    let symbol_map = load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("other").join("example-linux")),
        None,
    )
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("BE4E976C325246EE9D6B7847A670B2A90").unwrap()
    );
    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x1156))
        .unwrap();
    assert_eq!(symbol_map.resolve_symbol_name(result.symbol.name), "main");
    assert_eq!(
        symbol_map.lookup_sync(LookupAddress::Relative(0x1158)),
        None,
        "Gap between main and f"
    );
    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x1160))
        .unwrap();
    assert_eq!(symbol_map.resolve_symbol_name(result.symbol.name), "f");
}

#[test]
fn example_linux_plt_stubs() {
    // Regression test for #778: ELF PLT stubs should resolve to meaningful names
    // instead of synthesized fun_XXXX placeholders.
    let symbol_map = load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("other").join("example-linux")),
        None,
    )
    .unwrap();

    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x1020))
        .unwrap();
    assert_eq!(
        symbol_map.resolve_symbol_name(result.symbol.name),
        "<PLT header>"
    );

    let result = symbol_map
        .lookup_sync(LookupAddress::Relative(0x1030))
        .unwrap();
    assert_eq!(
        symbol_map.resolve_symbol_name(result.symbol.name),
        "printf@plt"
    );
}

#[test]
fn example_linux_fallback() {
    let symbol_map = load_symbol_map_from_location(
        FileLocationType(fixtures_dir().join("other").join("example-linux-fallback")),
        None,
    )
    .unwrap();
    assert_eq!(
        symbol_map.debug_id(),
        DebugId::from_breakpad("C3FC2519F439E42A970B693775586AA80").unwrap()
    );
    // no _stack_chk_fail@@GLIBC_2.4 please
    assert_eq!(symbol_map.lookup_sync(LookupAddress::Relative(0x6)), None);
}

#[test]
fn compare_snapshot() {
    let table = crate::get_table(
        &fixtures_dir().join("win64-ci").join("mozglue.pdb"),
        DebugId::from_breakpad("63C609072D3499F64C4C44205044422E1").ok(),
    )
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

fn synth_dyld_cache_root(
    v2_suffixes: Option<&[&str]>,
    v1_count: usize,
    with_symbols_file: bool,
) -> Vec<u8> {
    use std::mem::size_of;

    const SUBCACHE_ARRAY_OFFSET: usize = 0x800;
    const MAPPING_OFFSET_FIELD: usize = 0x10;
    const SUBCACHE_ARRAY_OFFSET_FIELD: usize = 0x188;
    const SUBCACHE_ARRAY_COUNT_FIELD: usize = 0x18c;
    const SYMBOL_FILE_UUID_FIELD: usize = 0x190;
    const V2_FILE_SUFFIX_FIELD: usize = 0x18;

    // The header size in mapping_offset determines the subcache entry version.
    let (mapping_offset, entry_size, entry_count) = match v2_suffixes {
        Some(suffixes) => (
            0x1d0u32,
            size_of::<DyldSubCacheEntryV2<Endianness>>(),
            suffixes.len(),
        ),
        None => (
            0x1c8u32,
            size_of::<DyldSubCacheEntryV1<Endianness>>(),
            v1_count,
        ),
    };

    let mut data = vec![0u8; SUBCACHE_ARRAY_OFFSET + entry_count * entry_size];
    let put_u32 = |data: &mut [u8], offset: usize, value: u32| {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes())
    };
    data[..16].copy_from_slice(b"dyld_v1  arm64e\0");
    put_u32(&mut data, MAPPING_OFFSET_FIELD, mapping_offset);
    put_u32(
        &mut data,
        SUBCACHE_ARRAY_OFFSET_FIELD,
        SUBCACHE_ARRAY_OFFSET as u32,
    );
    put_u32(&mut data, SUBCACHE_ARRAY_COUNT_FIELD, entry_count as u32);
    if with_symbols_file {
        data[SYMBOL_FILE_UUID_FIELD..SYMBOL_FILE_UUID_FIELD + 16].copy_from_slice(&[0xab; 16]);
    }
    if let Some(suffixes) = v2_suffixes {
        for (index, suffix) in suffixes.iter().enumerate() {
            let offset = SUBCACHE_ARRAY_OFFSET + index * entry_size + V2_FILE_SUFFIX_FIELD;
            data[offset..offset + suffix.len()].copy_from_slice(suffix.as_bytes());
        }
    }
    data
}

fn drive_dyld_cache_load(root_path: &Path) -> (Vec<String>, Result<(), Error>) {
    let mut load = DyldCacheLoad::<TestFileTypes>::new(
        FileLocationType(root_path.to_owned()),
        "/usr/lib/system/libsystem_c.dylib".to_string(),
    );
    let mut requested = Vec::new();
    while let LoadStep::NeedFile { location, .. } = load.poll() {
        let location = location.clone();
        requested.push(
            location
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let result = load_file(location);
        load.provide(result);
    }
    (requested, load.finish().map(|_| ()))
}

#[test]
fn dyld_cache_load_uses_header_listed_subcache_names() {
    let dir = tempfile::tempdir().unwrap();
    let suffixes = [".01", ".02.dylddata", ".03.dyldreadonly"];
    let root = dir.path().join("dyld_shared_cache_arm64e");
    std::fs::write(&root, synth_dyld_cache_root(Some(&suffixes), 0, true)).unwrap();
    for suffix in suffixes.iter().copied().chain([".symbols"]) {
        std::fs::write(
            dir.path().join(format!("dyld_shared_cache_arm64e{suffix}")),
            b"subcache",
        )
        .unwrap();
    }

    let (requested, result) = drive_dyld_cache_load(&root);
    assert_eq!(
        requested,
        [
            "dyld_shared_cache_arm64e",
            "dyld_shared_cache_arm64e.01",
            "dyld_shared_cache_arm64e.02.dylddata",
            "dyld_shared_cache_arm64e.03.dyldreadonly",
            "dyld_shared_cache_arm64e.symbols",
        ]
    );
    result.unwrap();
}

#[test]
fn dyld_cache_load_v1_numeric_subcache_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dyld_shared_cache_arm64e");
    std::fs::write(&root, synth_dyld_cache_root(None, 2, false)).unwrap();
    for suffix in [".1", ".2"] {
        std::fs::write(
            dir.path().join(format!("dyld_shared_cache_arm64e{suffix}")),
            b"subcache",
        )
        .unwrap();
    }

    let (requested, result) = drive_dyld_cache_load(&root);
    assert_eq!(
        requested,
        [
            "dyld_shared_cache_arm64e",
            "dyld_shared_cache_arm64e.1",
            "dyld_shared_cache_arm64e.2",
        ]
    );
    result.unwrap();
}

#[test]
fn dyld_cache_load_missing_listed_subcache_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dyld_shared_cache_arm64e");
    std::fs::write(
        &root,
        synth_dyld_cache_root(Some(&[".01", ".02.dylddata"]), 0, false),
    )
    .unwrap();
    std::fs::write(dir.path().join("dyld_shared_cache_arm64e.01"), b"subcache").unwrap();

    let (requested, result) = drive_dyld_cache_load(&root);
    assert!(requested
        .last()
        .unwrap()
        .ends_with("dyld_shared_cache_arm64e.02.dylddata"));
    match result {
        Err(Error::OpenFile(path, _)) => {
            assert!(path.ends_with(".02.dylddata"), "unexpected path: {path}")
        }
        other => panic!("expected OpenFile error for the missing subcache, got {other:?}"),
    }
}
