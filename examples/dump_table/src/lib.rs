use anyhow;
use memmap::MmapOptions;
use profiler_get_symbols::{
    self, CompactSymbolTable, FileAndPathHelper, FileAndPathHelperResult, GetSymbolsError,
    OwnedFileData,
};
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub async fn get_table(
    debug_name: &str,
    breakpad_id: Option<String>,
    symbol_directory: PathBuf,
) -> anyhow::Result<CompactSymbolTable> {
    let helper = Helper { symbol_directory };
    let table = get_symbols_retry_id(debug_name, breakpad_id, &helper).await?;
    Ok(table)
}

async fn get_symbols_retry_id(
    debug_name: &str,
    breakpad_id: Option<String>,
    helper: &Helper,
) -> anyhow::Result<CompactSymbolTable> {
    let breakpad_id = match breakpad_id {
        Some(breakpad_id) => breakpad_id,
        None => {
            // No breakpad ID was specified. get_compact_symbol_table always wants one, so we call it twice:
            // First, with a bogus breakpad ID ("<unspecified>"), and then again with the breakpad ID that
            // it expected.
            let result =
                profiler_get_symbols::get_compact_symbol_table(debug_name, "<unspecified>", helper)
                    .await;
            match result {
                Ok(table) => return Ok(table),
                Err(err) => match err {
                    GetSymbolsError::UnmatchedBreakpadId(expected, _) => {
                        eprintln!("Using breakpadID: {}", expected);
                        expected
                    }
                    GetSymbolsError::NoMatchMultiArch(errors) => {
                        // There's no one breakpad ID. We need the user to specify which one they want.
                        // Print out all potential breakpad IDs so that the user can pick.
                        let mut potential_ids: Vec<String> = vec![];
                        for err in errors {
                            if let GetSymbolsError::UnmatchedBreakpadId(expected, _) = err {
                                potential_ids.push(expected);
                            } else {
                                return Err(err.into());
                            }
                        }
                        eprintln!("This is a multi-arch container. Please specify one of the following breakpadIDs to pick a symbol table:");
                        for id in potential_ids {
                            println!(" - {}", id);
                        }
                        std::process::exit(0);
                    }
                    err => return Err(err.into()),
                },
            }
        }
    };
    Ok(profiler_get_symbols::get_compact_symbol_table(debug_name, &breakpad_id, helper).await?)
}

pub fn dump_table(
    w: &mut impl std::io::Write,
    table: CompactSymbolTable,
    full: bool,
) -> anyhow::Result<()> {
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
            eprintln!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
        }

        Box::pin(read_file_impl(path.to_owned()))
    }
}

#[cfg(test)]
mod test {

    use profiler_get_symbols::GetSymbolsError;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        let this_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        this_dir.join("..").join("..").join("fixtures")
    }

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
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors(sandbox::IPCInfo*, sandbox::CountedBuffer*)")
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
            Ok("sandbox::ProcessMitigationsWin32KDispatcher::EnumDisplayMonitors(sandbox::IPCInfo*, sandbox::CountedBuffer*)")
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
    }

    #[test]
    fn compare_snapshot() {
        let table = futures::executor::block_on(crate::get_table(
            "mozglue.pdb",
            Some(String::from("63C609072D3499F64C4C44205044422E2")),
            fixtures_dir().join("win64-ci"),
        ))
        .unwrap();
        let mut output: Vec<u8> = Vec::new();
        crate::dump_table(&mut output, table, true).unwrap();

        if false {
            let mut output_file = File::create(
                fixtures_dir()
                    .join("snapshots")
                    .join("output-win64-ci-mozglue.pdb.txt"),
            )
            .unwrap();
            output_file.write_all(&output).unwrap();
        }

        let mut snapshot_file = File::open(
            fixtures_dir()
                .join("snapshots")
                .join("win64-ci-mozglue.pdb.txt"),
        )
        .unwrap();
        let mut expected: Vec<u8> = Vec::new();
        snapshot_file.read_to_end(&mut expected).unwrap();
        assert_eq!(output, expected);
    }
}
