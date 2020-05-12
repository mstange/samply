use anyhow;
use memmap::MmapOptions;
use profiler_get_symbols;
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
    #[structopt(default_value = "<unspecified>")]
    breakpad_id: String,

    /// Path to the corresponding PDB file, if present
    #[structopt()]
    debug_path: Option<PathBuf>,
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
        profiler_get_symbols::get_compact_symbol_table(binary_data, debug_data, &opt.breakpad_id)?
    } else {
        profiler_get_symbols::get_compact_symbol_table(binary_data, binary_data, &opt.breakpad_id)?
    };
    println!("Found {} symbols.", table.addr.len());
    for (i, address) in table.addr.iter().enumerate() {
        let start_pos = table.index[i];
        let end_pos = table.index[i + 1];
        let symbol_bytes = &table.buffer[start_pos as usize..end_pos as usize];
        let symbol_string = std::str::from_utf8(symbol_bytes)?;
        println!("{:x} {}", address, symbol_string);
    }
    Ok(())
}
