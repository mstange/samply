use profiler_get_symbols::GetSymbolsError;
use std::path::PathBuf;
use structopt::StructOpt;

use dump_table::{dump_table, get_table};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "dump-table",
    about = "Get the symbol table for a debugName + breakpadId identifier."
)]
struct Opt {
    /// filename (just the filename, no path)
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

fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    let has_breakpad_id = opt.breakpad_id.is_some();
    let result = futures::executor::block_on(main_impl(
        &opt.debug_name,
        opt.breakpad_id,
        opt.symbol_directory,
        opt.full,
    ));
    let err = match result {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };
    match err.downcast::<GetSymbolsError>() {
        Ok(GetSymbolsError::NoMatchMultiArch(uuids, _)) if !has_breakpad_id => {
            // There's no one breakpad ID. We need the user to specify which one they want.
            // Print out all potential breakpad IDs so that the user can pick.
            eprintln!("This is a multi-arch container. Please specify one of the following breakpadIDs to pick a symbol table:");
            for id in uuids {
                println!(" - {}", id);
            }
            Ok(())
        }
        Ok(err) => Err(err.into()),
        Err(err) => Err(err),
    }
}

async fn main_impl(
    debug_name: &str,
    breakpad_id: Option<String>,
    symbol_directory: PathBuf,
    full: bool,
) -> anyhow::Result<()> {
    let table = get_table(debug_name, breakpad_id, symbol_directory).await?;
    dump_table(&mut std::io::stdout(), table, full)
}
