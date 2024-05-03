use std::path::PathBuf;

use clap::Parser;
use query_api::query_api;

#[derive(Parser)]
#[command(
    name = "dump-table",
    about = "Get the symbol table for a debugName + breakpadId identifier."
)]
struct Opt {
    /// Path to a directory that contains binaries and debug archives
    symbol_directory: PathBuf,

    /// A URL. Should always be /symbolicate/v5
    url: String,

    /// Request data, or path to file with request data if preceded by @ (like curl)
    request_json_or_filename: String,
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::parse();
    let request_json = if opt.request_json_or_filename.starts_with('@') {
        let filename = opt.request_json_or_filename.trim_start_matches('@');
        std::fs::read_to_string(filename)?
    } else {
        opt.request_json_or_filename
    };
    let response_json =
        futures::executor::block_on(query_api(&opt.url, &request_json, opt.symbol_directory));
    println!("{response_json}");
    Ok(())
}

#[test]
fn verify_cli() {
    use clap::CommandFactory;
    Opt::command().debug_assert()
}
