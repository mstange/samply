use profiler_get_symbols::{
    self, CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    OptionallySendFuture,
};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub async fn query_api(request_url: &str, request_json: &str, symbol_directory: PathBuf) -> String {
    let helper = Helper { symbol_directory };
    profiler_get_symbols::query_api(request_url, request_json, &helper).await
}
struct Helper {
    symbol_directory: PathBuf,
}

impl FileAndPathHelper for Helper {
    type F = memmap2::Mmap;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        _breakpad_id: &str,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
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
                    .join(&format!("{}.dSYM", debug_name))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            )));
        }

        // Finally, the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            self.symbol_directory.join(debug_name))
        ));

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
        &self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>>>> {
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
}

#[cfg(test)]
mod test {

    // use profiler_get_symbols::GetSymbolsError;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    use assert_json_diff::assert_json_eq;

    fn fixtures_dir() -> PathBuf {
        let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        this_dir.join("..").join("..").join("fixtures")
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

        let mut snapshot_file =
            File::open(fixtures_dir().join("snapshots").join(snapshot_filename)).unwrap();
        let mut expected: String = String::new();
        snapshot_file.read_to_string(&mut expected).unwrap();
        let expected_json: serde_json::Value = serde_json::from_str(&expected).unwrap();

        if output_json != expected_json {
            let mut output_file =
                File::create(fixtures_dir().join("snapshots").join(output_filename)).unwrap();
            output_file.write_all(output.as_bytes()).unwrap();
        }

        assert_json_eq!(output_json, expected_json);
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
    fn win64_ci_v5_legacy_snapshot_1() {
        compare_snapshot(
            "/symbolicate/v5-legacy",
            r#"{
                "memoryMap": [
                  [
                    "firefox.pdb",
                    "AA152DEB2D9B76084C4C44205044422E1"
                  ]
                ],
                "stacks": [
                  [
                    [0, 204776],
                    [0, 129423],
                    [0, 244290],
                    [0, 244219]
                  ]
                ]
              }"#,
            fixtures_dir().join("win64-ci"),
            "api-v5-legacy-win64-ci-1.txt",
            "output-api-v5-legacy-win64-ci-1.txt",
        );
    }

    #[test]
    fn win64_ci_v5_legacy_snapshot_2() {
        compare_snapshot(
            "/symbolicate/v5-legacy",
            r#"{
                "memoryMap": [
                  [
                    "mozglue.pdb",
                    "63C609072D3499F64C4C44205044422E1"
                  ]
                ],
                "stacks": [
                  [
                    [0, 244290],
                    [0, 244219],
                    [0, 237799]
                  ]
                ]
              }"#,
            fixtures_dir().join("win64-ci"),
            "api-v5-legacy-win64-ci-2.txt",
            "output-api-v5-legacy-win64-ci-2.txt",
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
    fn android32_v5_legacy_local() {
        compare_snapshot(
            "/symbolicate/v5-legacy",
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
            "api-v5-legacy-android32-local.txt",
            "output-api-v5-legacy-android32-local.txt",
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
}
