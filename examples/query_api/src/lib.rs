use memmap2::MmapOptions;
use profiler_get_symbols::{
    self, FileAndPathHelper, FileAndPathHelperResult, FileContents, OptionallySendFuture,
};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub async fn query_api(request_url: &str, request_json: &str, symbol_directory: PathBuf) -> String {
    let helper = Helper { symbol_directory };
    profiler_get_symbols::query_api(request_url, request_json, &helper).await
}

struct MmapFileContents(memmap2::Mmap);

impl FileContents for MmapFileContents {
    #[inline]
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    #[inline]
    fn read_bytes_at<'a>(&'a self, offset: u64, size: u64) -> FileAndPathHelperResult<&'a [u8]> {
        Ok(&self.0[offset as usize..][..size as usize])
    }
}
struct Helper {
    symbol_directory: PathBuf,
}

impl FileAndPathHelper for Helper {
    type F = MmapFileContents;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        _breakpad_id: &str,
    ) -> FileAndPathHelperResult<Vec<PathBuf>> {
        let mut paths = vec![];

        // Also consider .so.dbg files in the symbol directory.
        if debug_name.ends_with(".so") {
            let debug_debug_name = format!("{}.dbg", debug_name);
            paths.push(self.symbol_directory.join(debug_debug_name));
        }

        // And dSYM packages.
        if !debug_name.ends_with(".pdb") {
            paths.push(
                self.symbol_directory
                    .join(&format!("{}.dSYM", debug_name))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            );
        }

        // Finally, the file itself.
        paths.push(self.symbol_directory.join(debug_name));

        Ok(paths)
    }

    fn open_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>>>> {
        async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<MmapFileContents> {
            eprintln!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
        }

        // See if this file exists in self.symbol_directory.
        // For example, when looking up object files referenced by mach-O binaries,
        // we want to take the object files from the symbol directory if they exist,
        // rather than from the original path.
        let mut path = path.to_owned();
        if let Some(filename) = path.file_name() {
            let redirected_path = self.symbol_directory.join(filename);
            if let Ok(_) = std::fs::metadata(&redirected_path) {
                // redirected_path exists!
                eprintln!("Redirecting {:?} to {:?}", &path, &redirected_path);
                path = redirected_path;
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

        if false {
            let mut output_file =
                File::create(fixtures_dir().join("snapshots").join(output_filename)).unwrap();
            output_file.write_all(&output.as_bytes()).unwrap();
        }

        let mut snapshot_file =
            File::open(fixtures_dir().join("snapshots").join(snapshot_filename)).unwrap();
        let mut expected: String = String::new();
        snapshot_file.read_to_string(&mut expected).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn win64_ci_v5_snapshot_1() {
        compare_snapshot(
            "/symbolicate/v5",
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
            "api-v5-win64-ci-1.txt",
            "output-api-v5-win64-ci-1.txt",
        );
    }

    #[test]
    fn win64_ci_v5_snapshot_2() {
        compare_snapshot(
            "/symbolicate/v5",
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
            "api-v5-win64-ci-2.txt",
            "output-api-v5-win64-ci-2.txt",
        );
    }

    #[test]
    fn win64_ci_v6_snapshot() {
        compare_snapshot(
            "/symbolicate/v6a1",
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
            "api-v6-win64-ci.txt",
            "output-api-v6-win64-ci.txt",
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
    fn android32_v6_local() {
        compare_snapshot(
            "/symbolicate/v6a1",
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
            "api-v6-android32-local.txt",
            "output-api-v6-android32-local.txt",
        );
    }
}
