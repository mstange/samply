use anyhow;
use futures;
use std::path::PathBuf;
use structopt::StructOpt;

use dump_table::{get_table, dump_table};

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
    futures::executor::block_on(main_impl(
        &opt.debug_name,
        opt.breakpad_id,
        opt.symbol_directory,
        opt.full,
    ))
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
