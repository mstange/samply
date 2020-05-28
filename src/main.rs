use std::path::PathBuf;
use structopt::StructOpt;

use profiler_symbol_server::start_server;


#[derive(Debug, StructOpt)]
#[structopt(
    name = "profiler-symbol-server",
    about = "A local webserver that serves a profile and symbol information."
)]
struct Opt {
    /// Open the profiler in your default browser.
    #[structopt(short, long)]
    open: bool,

    /// The profile file that should be served.
    #[structopt(parse(from_os_str))]
    file: PathBuf,
}

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
    start_server(&opt.file, opt.open).await;
}
