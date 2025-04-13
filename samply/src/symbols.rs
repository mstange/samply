use std::path::Path;
use std::sync::Arc;

use platform_dirs::AppDirs;
use samply_quota_manager::QuotaManager;
use wholesym::{SymbolManager, SymbolManagerConfig};

use crate::name::SAMPLY_NAME;
use crate::shared::prop_types::SymbolProps;
use crate::shared::symbol_manager_observer::SamplySymbolManagerObserver;

fn create_quota_manager(symbols_dir: &Path) -> Option<QuotaManager> {
    let db_path = symbols_dir.parent().unwrap().join("symbols.db");

    if let Err(e) = std::fs::create_dir_all(symbols_dir) {
        log::error!("Could not create symbol cache directory {symbols_dir:?}: {e}");
        return None;
    }

    const TEN_GIGABYTES_AS_BYTES: u64 = 10 * 1000 * 1000 * 1000;
    const TWO_WEEKS_AS_SECONDS: u64 = 2 * 7 * 24 * 60 * 60;
    let quota_manager = match QuotaManager::new(symbols_dir, &db_path) {
        Ok(quota_manager) => quota_manager,
        Err(e) => {
            log::error!(
                "Could not create QuotaManager with symbol cache database {db_path:?}: {e}"
            );
            return None;
        }
    };
    quota_manager.set_max_total_size(Some(TEN_GIGABYTES_AS_BYTES));
    quota_manager.set_max_age(Some(TWO_WEEKS_AS_SECONDS));
    Some(quota_manager)
}

fn create_symbol_manager_config_and_quota_manager(
    symbol_props: SymbolProps,
) -> (SymbolManagerConfig, Option<QuotaManager>) {
    let _config_dir = AppDirs::new(Some(SAMPLY_NAME), true).map(|dirs| dirs.config_dir);
    let cache_base_dir = AppDirs::new(Some(SAMPLY_NAME), false).map(|dirs| dirs.cache_dir);
    let symbols_dir = cache_base_dir.map(|cache_base_dir| cache_base_dir.join("symbols"));
    let symbols_dir = symbols_dir.as_deref();

    let mut config = SymbolManagerConfig::new()
        .respect_nt_symbol_path(true)
        .use_debuginfod(std::env::var("SAMPLY_USE_DEBUGINFOD").is_ok())
        .use_spotlight(true);

    let quota_manager = match &symbols_dir {
        Some(symbols_dir) => create_quota_manager(symbols_dir),
        None => None,
    };

    if let Some(symbols_dir) = symbols_dir {
        config = config.debuginfod_cache_dir_if_not_installed(symbols_dir.join("debuginfod"));
    }

    // TODO: Read symbol server config from some kind of config file
    // TODO: On Windows, put https://msdl.microsoft.com/download/symbols into the config file.

    // Configure symbol servers and cache directories based on the information in the SymbolProps.

    let breakpad_symbol_cache_dir = symbol_props
        .breakpad_symbol_cache
        .or_else(|| Some(symbols_dir?.join("breakpad")));
    if let Some(cache_dir) = breakpad_symbol_cache_dir {
        for base_url in symbol_props.breakpad_symbol_server {
            config = config.breakpad_symbol_server(base_url, &cache_dir)
        }
        for dir in symbol_props.breakpad_symbol_dir {
            config = config.breakpad_symbol_dir(dir);
        }
        if let Some(symbols_dir) = symbols_dir {
            let breakpad_symindex_cache_dir = symbols_dir.join("breakpad-symindex");
            config = config.breakpad_symindex_cache_dir(breakpad_symindex_cache_dir);
        }
    }

    let windows_symbol_cache_dir = symbol_props
        .windows_symbol_cache
        .or_else(|| Some(symbols_dir?.join("windows")));
    if let Some(cache_dir) = windows_symbol_cache_dir {
        for base_url in symbol_props.windows_symbol_server {
            config = config.windows_symbol_server(base_url, &cache_dir)
        }
    }

    if let Some(binary_cache) = symbol_props.simpleperf_binary_cache {
        config = config.simpleperf_binary_cache_dir(binary_cache);
    }

    for dir in symbol_props.symbol_dir {
        config = config.extra_symbol_directory(dir);
    }

    (config, quota_manager)
}

pub fn create_symbol_manager_and_quota_manager(
    symbol_props: SymbolProps,
    verbose: bool,
) -> (SymbolManager, Option<QuotaManager>) {
    let (config, quota_manager) = create_symbol_manager_config_and_quota_manager(symbol_props);
    let mut symbol_manager = SymbolManager::with_config(config);
    let notifiers = match &quota_manager {
        Some(mgr) => vec![mgr.notifier()],
        None => vec![],
    };
    if let Some(mgr) = &quota_manager {
        // Enforce size limit and delete old files.
        // Not sure if it's wise to do this at startup unconditionally.
        // I can see that it might be annoying in the following case:
        //  1. Download lots of symbols.
        //  2. Don't run samply for two weeks.
        //  3. Capture a new profile.
        //  4. The server starts, all symbols get deleted because they're over two weeks old.
        //  5. The browser opens, the profiler requests symbols, all symbols are downloaded again.
        // Should we delay the first eviction? But then we might delay it for too long.
        mgr.notifier().trigger_eviction_if_needed();
    }
    symbol_manager.set_observer(Some(Arc::new(SamplySymbolManagerObserver::new(
        verbose, notifiers,
    ))));
    (symbol_manager, quota_manager)
}
