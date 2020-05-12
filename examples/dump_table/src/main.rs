use anyhow;
use memmap::MmapOptions;
use profiler_get_symbols::{self, CompactSymbolTable, GetSymbolsError};
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "dump-table", about = "Dump the symbol table of a binary.")]
struct Opt {
    /// Path to the binary
    #[structopt()]
    binary_path: PathBuf,

    /// Breakpad ID of the binary
    #[structopt()]
    breakpad_id: Option<String>,

    /// Path to the corresponding PDB file, if present
    #[structopt()]
    debug_path: Option<PathBuf>,

    /// When specified, print the entire symbol table.
    #[structopt(short, long)]
    full: bool,
}

fn get_symbols_retry_id(
    binary_data: &[u8],
    debug_data: &[u8],
    breakpad_id: Option<String>,
) -> anyhow::Result<CompactSymbolTable> {
    let breakpad_id = match breakpad_id {
        Some(breakpad_id) => breakpad_id,
        None => {
            // No breakpad ID was specified. get_compact_symbol_table always wants one, so we call it twice:
            // First, with a bogus breakpad ID ("<unspecified>"), and then again with the breakpad ID that
            // it expected.
            let result = profiler_get_symbols::get_compact_symbol_table(
                binary_data,
                debug_data,
                "<unspecified>",
            );
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
    Ok(profiler_get_symbols::get_compact_symbol_table(
        binary_data,
        debug_data,
        &breakpad_id,
    )?)
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    let binary_file = File::open(opt.binary_path)?;
    let binary_mmap = unsafe { MmapOptions::new().map(&binary_file)? };
    let binary_data = &*binary_mmap;
    let table = if let Some(debug_path) = opt.debug_path {
        let debug_file = File::open(debug_path)?;
        let debug_mmap = unsafe { MmapOptions::new().map(&debug_file)? };
        let debug_data = &*debug_mmap;
        get_symbols_retry_id(binary_data, debug_data, opt.breakpad_id)?
    } else {
        get_symbols_retry_id(binary_data, binary_data, opt.breakpad_id)?
    };
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
