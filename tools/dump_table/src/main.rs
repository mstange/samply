use samply_symbols::{debugid::DebugId, Error};
use std::path::{Path, PathBuf};
use structopt::StructOpt;

use dump_table::{dump_table, get_table_for_binary};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "dump-table",
    about = "Get the symbol table for a debugName + breakpadId identifier."
)]
struct Opt {
    /// binary path (just the filename, no path)
    #[structopt()]
    binary_path: PathBuf,

    /// Breakpad ID of the binary
    #[structopt()]
    breakpad_id: Option<String>,

    /// When specified, print the entire symbol table.
    #[structopt(short, long)]
    full: bool,
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();
    let result =
        futures::executor::block_on(main_impl(&opt.binary_path, opt.breakpad_id, opt.full));
    match result {
        Ok(()) => Ok(()),
        Err(Error::NoDisambiguatorForFatArchive(members)) => {
            // There's no one breakpad ID. We need the user to specify which one they want.
            // Print out all potential breakpad IDs so that the user can pick.
            eprintln!("This is a multi-arch container. Please specify one of the following breakpadIDs to pick a symbol table:");
            for m in members {
                if let Some(uuid) = m.uuid {
                    println!(" - {}", DebugId::from_uuid(uuid).breakpad());
                }
            }
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

async fn main_impl(
    binary_path: &Path,
    breakpad_id: Option<String>,
    full: bool,
) -> Result<(), Error> {
    let debug_id = breakpad_id
        .as_deref()
        .and_then(|debug_id| DebugId::from_breakpad(debug_id).ok());
    let table = get_table_for_binary(binary_path, debug_id).await?;
    dump_table(&mut std::io::stdout(), table, full).unwrap();
    Ok(())
}
