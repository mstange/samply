use std::path::PathBuf;

pub use wholesym::debugid;
use wholesym::{QueryApiJsonResult, SymbolManager, SymbolManagerConfig};

pub async fn query_api(
    request_url: &str,
    request_json: &str,
    symbol_directory: PathBuf,
) -> QueryApiJsonResult {
    let config = SymbolManagerConfig::new().extra_symbol_directory(symbol_directory);
    let symbol_manager = SymbolManager::with_config(config);
    symbol_manager
        .query_json_api(request_url, request_json)
        .await
}
