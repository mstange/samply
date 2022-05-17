use std::path::PathBuf;
use structopt::StructOpt;

use profiler_symbol_server::{start_server, PortSelection};
use profiler_symbol_server::symsrv::get_symbol_path_from_environment;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "profiler-symbol-server",
    about = "A local webserver that serves a profile and symbol information.",
    usage = "profiler-symbol-server [<file>]"
)]
struct Opt {
    /// Do not open the profiler UI.
    #[structopt(short, long)]
    no_open: bool,

    /// The port to use for the local web server.
    #[structopt(short, long, default_value = "3000+")]
    port: String,

    /// The profile file that should be served.
    #[structopt(parse(from_os_str))]
    file: Option<PathBuf>,

    /// Print debugging messages.
    #[structopt(short, long)]
    verbose: bool,
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
    let symbol_path =
        get_symbol_path_from_environment("srv**https://msdl.microsoft.com/download/symbols");
    start_server(
        opt.file.as_deref(),
        port_selection,
        symbol_path,
        opt.verbose,
        !opt.no_open,
    )
    .await;
}
