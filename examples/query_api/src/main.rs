use anyhow;
use futures;
use memmap::MmapOptions;
use profiler_get_symbols::{self, FileAndPathHelper, FileAndPathHelperResult, OwnedFileData};
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "dump-table",
    about = "Get the symbol table for a debugName + breakpadId identifier."
)]
struct Opt {
    /// Path to a directory that contains binaries and debug archives
    #[structopt()]
    symbol_directory: PathBuf,

    /// A URL. Should always be /symbolicate/v5
    #[structopt()]
    url: String,

    /// Request data, or path to file with request data if preceded by @ (like curl)
    #[structopt()]
    request_json_or_filename: String,
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    let request_json = if opt.request_json_or_filename.starts_with('@') {
        let filename = opt.request_json_or_filename.trim_start_matches('@');
        std::fs::read_to_string(filename)?
    } else {
        opt.request_json_or_filename
    };
    let response_json =
        futures::executor::block_on(query_api(&opt.url, &request_json, opt.symbol_directory));
    println!("{}", response_json);
    Ok(())
}

async fn query_api(request_url: &str, request_json: &str, symbol_directory: PathBuf) -> String {
    let helper = Helper { symbol_directory };
    profiler_get_symbols::query_api(request_url, request_json, &helper).await
}

struct MmapFileContents(memmap::Mmap);

impl OwnedFileData for MmapFileContents {
    fn get_data(&self) -> &[u8] {
        &*self.0
    }
}

struct Helper {
    symbol_directory: PathBuf,
}

impl FileAndPathHelper for Helper {
    type FileContents = MmapFileContents;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        _breakpad_id: &str,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>>>> {
        async fn to_future(
            res: FileAndPathHelperResult<Vec<PathBuf>>,
        ) -> FileAndPathHelperResult<Vec<PathBuf>> {
            res
        }
        Box::pin(to_future(Ok(vec![self.symbol_directory.join(debug_name)])))
    }

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::FileContents>>>> {
        async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<MmapFileContents> {
            println!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
        }

        Box::pin(read_file_impl(path.to_owned()))
    }
}

#[cfg(test)]
mod test {

    // use profiler_get_symbols::GetSymbolsError;
    use std::path::PathBuf;
    use std::fs::File;
    use std::io::{Read, Write};

    fn fixtures_dir() -> PathBuf {
        let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        this_dir.join("..").join("..").join("fixtures")
    }

    fn compare_snapshot(request_url: &str, request_json: &str, symbol_directory: PathBuf, snapshot_filename: &str, output_filename: &str) {
        let output = futures::executor::block_on(crate::query_api(
            request_url,
            request_json,
            symbol_directory,
        ));

        if false {
            let mut output_file = File::create(
                fixtures_dir()
                    .join("snapshots")
                    .join(output_filename),
            )
            .unwrap();
            output_file.write_all(&output.as_bytes()).unwrap();
        }

        let mut snapshot_file = File::open(
            fixtures_dir()
                .join("snapshots")
                .join(snapshot_filename),
        )
        .unwrap();
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
                    "AA152DEB2D9B76084C4C44205044422E2"
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
                    "63C609072D3499F64C4C44205044422E2"
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
                    "AA152DEB2D9B76084C4C44205044422E2"
                  ],
                  [
                    "mozglue.pdb",
                    "63C609072D3499F64C4C44205044422E2"
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
    #[should_panic] // https://github.com/gimli-rs/addr2line/issues/167
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
