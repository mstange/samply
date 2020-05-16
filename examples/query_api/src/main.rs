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

    fn fixtures_dir() -> PathBuf {
        let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        this_dir.join("..").join("..").join("fixtures")
    }
    /*
    #[test]
    fn successful_pdb() {
        let result = futures::executor::block_on(crate::get_table(
            "firefox.pdb",
            Some(String::from("AA152DEB2D9B76084C4C44205044422E2")),
            fixtures_dir().join("win64-ci"),
        ));
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.addr.len(), 1286);
        assert_eq!(result.addr[776], 0x31fc0);
        assert_eq!(
            std::str::from_utf8(
                &result.buffer[result.index[776] as usize..result.index[777] as usize]
            ),
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors")
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
        assert_eq!(result.addr.len(), 1286);
        assert_eq!(result.addr[776], 0x31fc0);
        assert_eq!(
            std::str::from_utf8(
                &result.buffer[result.index[776] as usize..result.index[777] as usize]
            ),
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors")
        );
    }

    #[test]
    fn unsuccessful_pdb_wrong_id() {
        let result = futures::executor::block_on(crate::get_table(
            "firefox.pdb",
            Some(String::from("AA152DEBFFFFFFFFFFFFFFFFF044422E2")),
            fixtures_dir().join("win64-ci"),
        ));
        assert!(result.is_err());
        let err = match result {
            Ok(_) => panic!("Shouldn't have succeeded with wrong breakpad ID"),
            Err(err) => err,
        };
        let err = match err.downcast::<GetSymbolsError>() {
            Ok(err) => err,
            Err(_) => panic!("wrong error type"),
        };
        match err {
            GetSymbolsError::UnmatchedBreakpadId(expected, actual) => {
                assert_eq!(expected, "AA152DEB2D9B76084C4C44205044422E2");
                assert_eq!(actual, "AA152DEBFFFFFFFFFFFFFFFFF044422E2");
            }
            _ => panic!("wrong GetSymbolsError subtype"),
        }
    }*/
}
