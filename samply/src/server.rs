use samply_server::symsrv::get_symbol_path_from_environment;
use samply_server::{start_server, PortSelection};

use std::path::Path;

#[derive(Clone, Debug)]
pub struct ServerProps {
    pub port_selection: PortSelection,
    pub verbose: bool,
    pub open_in_browser: bool,
}

#[tokio::main]
pub async fn start_server_main(file: &Path, props: ServerProps) {
    start_server(
        Some(file),
        props.port_selection,
        get_symbol_path_from_environment("srv**https://msdl.microsoft.com/download/symbols"),
        props.verbose,
        props.open_in_browser,
    )
    .await;
}
