use assert_json_diff::assert_json_eq;
use futures::Future;
pub use samply_api::debugid::DebugId;
use samply_api::samply_symbols::{
    CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
};
use samply_api::Api;
use samply_symbols::{LibraryInfo, SymbolManager};

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub async fn query_api(request_url: &str, request_json: &str, symbol_directory: PathBuf) -> String {
    let helper = Helper { symbol_directory };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let api = Api::new(&symbol_manager);
    api.query_api(request_url, request_json).await
}
struct Helper {
    symbol_directory: PathBuf,
}

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = memmap2::Mmap;
    type OpenFileFuture = Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let debug_name = match library_info.debug_name.as_deref() {
            Some(debug_name) => debug_name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Check .so.dbg files in the symbol directory.
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
                    .join(&format!("{}.dSYM", debug_name))
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
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<memmap2::Mmap> {
            eprintln!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(unsafe { memmap2::MmapOptions::new().map(&file)? })
        }

        let mut path = match location {
            FileLocation::Path(path) => path.clone(),
            FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
        };
        if !path.starts_with(&self.symbol_directory) {
            // See if this file exists in self.symbol_directory.
            // For example, when looking up object files referenced by mach-O binaries,
            // we want to take the object files from the symbol directory if they exist,
            // rather than from the original path.
            if let Some(filename) = path.file_name() {
                let redirected_path = self.symbol_directory.join(filename);
                if std::fs::metadata(&redirected_path).is_ok() {
                    // redirected_path exists!
                    eprintln!("Redirecting {:?} to {:?}", &path, &redirected_path);
                    path = redirected_path;
                }
            }
        }

        Box::pin(read_file_impl(path))
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

fn compare_snapshot(
    request_url: &str,
    request_json: &str,
    symbol_directory: PathBuf,
    snapshot_filename: &str,
    output_filename: &str,
) {
    let output = futures::executor::block_on(crate::query_api(
        request_url,
        request_json,
        symbol_directory,
    ));

    let output_json: serde_json::Value = serde_json::from_str(&output).unwrap();

    let mut expected_json: Option<serde_json::Value> = None;
    if let Ok(mut snapshot_file) =
        File::open(fixtures_dir().join("snapshots").join(snapshot_filename))
    {
        let mut expected: String = String::new();
        snapshot_file.read_to_string(&mut expected).unwrap();
        expected_json = Some(serde_json::from_str(&expected).unwrap());
    }

    if expected_json.as_ref() != Some(&output_json) {
        let mut output_file =
            File::create(fixtures_dir().join("snapshots").join(output_filename)).unwrap();
        output_file.write_all(output.as_bytes()).unwrap();
    }

    match expected_json {
        Some(expected_json) => assert_json_eq!(output_json, expected_json),
        None => panic!("No snapshot found"),
    }
}

#[test]
fn win64_local_v5_snapshot_1() {
    // This gets the symbols from the DLL exports, not from the PDB.
    compare_snapshot(
        "/symbolicate/v5",
        r#"{
                "memoryMap": [
                  [
                    "mozglue.dll",
                    "B3CC644ECC086E044C4C44205044422E1"
                  ]
                ],
                "stacks": [
                  [
                    [0, 214644]
                  ]
                ]
              }"#,
        fixtures_dir().join("win64-local"),
        "api-v5-win64-local-1.txt",
        "output-api-v5-win64-local-1.txt",
    );
}

#[test]
fn win64_ci_v5_snapshot() {
    compare_snapshot(
        "/symbolicate/v5",
        r#"{
                "memoryMap": [
                  [
                    "firefox.pdb",
                    "AA152DEB2D9B76084C4C44205044422E1"
                  ],
                  [
                    "mozglue.pdb",
                    "63C609072D3499F64C4C44205044422E1"
                  ]
                ],
                "stacks": [
                  [
                    [0, 204776],
                    [0, 129423],
                    [0, 244290],
                    [0, 244219],
                    [1, 244290],
                    [1, 244219],
                    [1, 237799]
                  ]
                ]
              }"#,
        fixtures_dir().join("win64-ci"),
        "api-v5-win64-ci.txt",
        "output-api-v5-win64-ci.txt",
    );
}

#[test]
fn android32_v5_local() {
    compare_snapshot(
        "/symbolicate/v5",
        r#"{
                "memoryMap": [
                  [
                    "libmozglue.so",
                    "0CE47B7C29F27CED55C41233B93EBA450"
                  ]
                ],
                "stacks": [
                  [
                    [0, 247618],
                    [0, 685896],
                    [0, 686768]
                  ]
                ]
              }"#,
        fixtures_dir().join("android32-local"),
        "api-v5-android32-local.txt",
        "output-api-v5-android32-local.txt",
    );
}

#[test]
fn stripped_macos() {
    // The address 232505 (0x38c39) is inside the __stub_helper section.
    // It should not be considered part of the last function in the __text section.
    // Returning no symbol at all for it is better than returning fun_384d0.
    // (0x384d0 being the start address of the last function in __text)
    compare_snapshot(
        "/symbolicate/v5",
        r#"{
                "memoryMap": [
                  [
                    "libsoftokn3.dylib",
                    "F7DE6E25737B3B1885A5079DC41D77B40"
                  ]
                ],
                "stacks": [
                  [
                    [0, 230071],
                    [0, 232505]
                  ]
                ]
              }"#,
        fixtures_dir().join("macos-ci"),
        "api-v5-stripped-macos.txt",
        "output-api-v5-stripped-macos.txt",
    );
}

#[test]
fn win_exe() {
    // The address 158574 (0x26b6e) is inside a leaf function which does not
    // allocate any stack space. These functions don't require unwind info, and
    // consequently are not listed in the .pdata section.
    // This address should not be considered to be part of the closest function
    // with unwind info; it is better to treat it as an unknown address than to
    // lump it in with the placeholder function fun_26ab0.
    // The actual function containing 0x26b6e starts at 0x26b60 and ends at 0x26b7d
    // but we currently don't know how to obtain this information from the exe.
    compare_snapshot(
        "/symbolicate/v5",
        r#"{
                "memoryMap": [
                  [
                    "updater.exe",
                    "5C08299576CB004F4C4C44205044422E1"
                  ]
                ],
                "stacks": [
                  [
                    [0, 27799],
                    [0, 158574]
                  ]
                ]
              }"#,
        fixtures_dir().join("win64-local"),
        "api-v5-win-exe.txt",
        "output-api-v5-win-exe.txt",
    );
}
