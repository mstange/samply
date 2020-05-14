use anyhow;
use futures;
use memmap::MmapOptions;
use profiler_get_symbols::{
    self, CompactSymbolTable, FileAndPathHelper, FileAndPathHelperResult, GetSymbolsError,
    OwnedFileData,
};
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
    /// debugName identifier
    #[structopt()]
    debug_name: String,

    /// Path to a directory that contains binaries and debug archives
    #[structopt()]
    symbol_directory: PathBuf,

    /// Breakpad ID of the binary
    #[structopt()]
    breakpad_id: Option<String>,

    /// When specified, print the entire symbol table.
    #[structopt(short, long)]
    full: bool,
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
            let result = profiler_get_symbols::get_compact_symbol_table_async(
                debug_name,
                "<unspecified>",
                helper,
            )
            .await;
            match result {
                Ok(table) => return Ok(table),
                Err(err) => match err {
                    GetSymbolsError::UnmatchedBreakpadId(expected, _) => {
                        println!("Using breakpadID: {}", expected);
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
                        println!("This is a multi-arch container. Please specify one of the following breakpadIDs to pick a symbol table:");
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
    Ok(
        profiler_get_symbols::get_compact_symbol_table_async(debug_name, &breakpad_id, helper)
            .await?,
    )
}

fn main() -> anyhow::Result<()> {
    futures::executor::block_on(main_impl())
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
        Box::pin(get_candidate_paths_for_binary_or_pdb_impl(
            debug_name.to_owned(),
            self.symbol_directory.clone(),
        ))
    }

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::FileContents>>>> {
        Box::pin(read_file_impl(path.to_owned()))
    }
}

async fn get_candidate_paths_for_binary_or_pdb_impl(
    debug_name: String,
    symbol_directory: PathBuf,
) -> FileAndPathHelperResult<Vec<PathBuf>> {
    Ok(vec![symbol_directory.join(&debug_name)])
}

async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<MmapFileContents> {
    println!("Reading file {:?}", &path);
    let file = File::open(&path)?;
    Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
}

async fn main_impl() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    let helper = Helper {
        symbol_directory: opt.symbol_directory,
    };
    let table = get_symbols_retry_id(&opt.debug_name, opt.breakpad_id, &helper).await?;
    println!("Found {} symbols.", table.addr.len());
    for (i, address) in table.addr.iter().enumerate() {
        if i >= 15 && !opt.full {
            println!(
                "and {} more symbols. Pass --full to print the full list.",
                table.addr.len() - i
            );
            break;
        }

        let start_pos = table.index[i];
        let end_pos = table.index[i + 1];
        let symbol_bytes = &table.buffer[start_pos as usize..end_pos as usize];
        let symbol_string = std::str::from_utf8(symbol_bytes)?;
        println!("{:x} {}", address, symbol_string);
    }
    Ok(())
}
