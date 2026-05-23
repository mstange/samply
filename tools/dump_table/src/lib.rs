use std::borrow::Cow;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub use wholesym::debugid;
use wholesym::debugid::DebugId;
use wholesym::{Error, SymbolManager, SymbolManagerConfig, SymbolMap};

pub async fn get_symbol_map_for_binary(
    binary_path: &Path,
    debug_id: Option<DebugId>,
) -> Result<SymbolMap, Error> {
    let mut config = SymbolManagerConfig::new();
    if let Some(parent) = binary_path.parent() {
        config = config.extra_symbol_directory(parent);
    }
    let symbol_manager = SymbolManager::with_config(config);
    let disambiguator = debug_id.map(wholesym::MultiArchDisambiguator::DebugId);
    symbol_manager
        .load_symbol_map_for_binary_at_path(binary_path, disambiguator)
        .await
}

pub async fn get_symbol_map_for_debug_name_and_id(
    debug_name: &str,
    debug_id: DebugId,
    symbol_directory: PathBuf,
) -> Result<SymbolMap, Error> {
    let config = SymbolManagerConfig::new().extra_symbol_directory(symbol_directory);
    let symbol_manager = SymbolManager::with_config(config);
    symbol_manager.load_symbol_map(debug_name, debug_id).await
}

pub fn dump_symbol_map(
    w: &mut impl Write,
    symbol_map: &SymbolMap,
    full: bool,
) -> anyhow::Result<()> {
    let mut w = BufWriter::new(w);
    let count = symbol_map.symbol_count();
    writeln!(w, "Found {} symbols.", count)?;
    let symbols: Vec<(u32, Cow<'_, str>)> = symbol_map.iter_symbols().collect();
    for (i, (address, name)) in symbols.iter().enumerate() {
        if i >= 15 && !full {
            writeln!(
                w,
                "and {} more symbols. Pass --full to print the full list.",
                symbols.len() - i
            )?;
            break;
        }
        writeln!(w, "{address:x} {name}")?;
    }
    Ok(())
}
