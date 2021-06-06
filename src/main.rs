use std::path::PathBuf;
use structopt::StructOpt;

use profiler_symbol_server::{start_server, PortSelection};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "profiler-symbol-server",
    about = "A local webserver that serves a profile and symbol information.",
    usage = "profiler-symbol-server [--open] <file>"
)]
struct Opt {
    /// Open the profiler in your default browser.
    #[structopt(long)]
    open: bool,

    /// The port to use for the local web server.
    #[structopt(short, long, default_value = "3000+")]
    port: String,

    /// The profile file that should be served.
    #[structopt(parse(from_os_str))]
    file: PathBuf,
}

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
    let port_selection = match PortSelection::try_from_str(&opt.port) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "Could not parse port as <u16> or <u16>+, got port {}, error: {}",
                opt.port, e
            );
            std::process::exit(1)
        }
    };
    start_server(&opt.file, port_selection, opt.open).await;
}
