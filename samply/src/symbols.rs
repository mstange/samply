use std::path::{Path, PathBuf};
use std::sync::Arc;

use samply_quota_manager::{QuotaManager, QuotaManagerNotifier};
use wholesym::{SymbolManager, SymbolManagerConfig};

use crate::config::{ConfigPaths, EvictionConfig};
use crate::shared::prop_types::SymbolProps;
use crate::shared::symbol_manager_observer::SamplySymbolManagerObserver;

/// The base directory of the symbol cache: the configured directory, or the
/// platform default. `None` if no home directory can be determined.
///
/// Callers which manage more than one [`SymbolManager`] use this to decide
/// which of them can share a [`QuotaManager`].
pub fn resolved_symbol_cache_dir(symbol_props: &SymbolProps) -> Option<PathBuf> {
    match &symbol_props.symbol_cache_dir {
        Some(dir) => Some(dir.clone()),
        None => Some(ConfigPaths::detect()?.default_symbol_cache_dir),
    }
}

/// Creates the [`QuotaManager`] which enforces the size and age limits of the
/// symbol cache directory, creating the directory if needed.
///
/// Must be called from within a tokio runtime context, because the
/// [`QuotaManager`] spawns its eviction task.
///
/// There must only be one of these per directory within a process. A process
/// which serves multiple profiles creates one per distinct cache directory and
/// shares its notifier between the symbol managers using that directory.
pub fn create_symbol_cache_quota_manager(
    symbols_dir: &Path,
    eviction: &EvictionConfig,
) -> Option<QuotaManager> {
    let db_path = symbols_dir.parent().unwrap().join("symbols.db");

    if let Err(e) = std::fs::create_dir_all(symbols_dir) {
        log::error!("Could not create symbol cache directory {symbols_dir:?}: {e}");
        return None;
    }

    let quota_manager = match QuotaManager::new(symbols_dir, &db_path) {
        Ok(quota_manager) => quota_manager,
        Err(e) => {
            log::error!(
                "Could not create QuotaManager with symbol cache database {db_path:?}: {e}"
            );
            return None;
        }
    };
    eviction.apply_to(&quota_manager);
    Some(quota_manager)
}

fn create_symbol_manager_config(symbol_props: SymbolProps) -> SymbolManagerConfig {
    let symbols_dir = resolved_symbol_cache_dir(&symbol_props);
    let symbols_dir = symbols_dir.as_deref();

    let use_debuginfod =
        symbol_props.use_debuginfod || std::env::var_os("SAMPLY_USE_DEBUGINFOD").is_some();
    let mut config = SymbolManagerConfig::new()
        .respect_nt_symbol_path(symbol_props.respect_nt_symbol_path)
        .use_debuginfod(use_debuginfod)
        .use_spotlight(true);

    if let Some(symbols_dir) = symbols_dir {
        config = config.debuginfod_cache_dir_if_not_installed(symbols_dir.join("debuginfod"));
    }

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

    config
}

/// Creates a [`SymbolManager`] for one profile.
///
/// `notifiers` are told about every file the symbol manager reads or writes in
/// the symbol cache, so that the cache's [`QuotaManager`] can keep its
/// inventory up to date. Pass the notifier of the [`QuotaManager`] which manages
/// [`resolved_symbol_cache_dir`] for these props, if there is one.
pub fn create_symbol_manager(
    symbol_props: SymbolProps,
    verbose: bool,
    notifiers: Vec<QuotaManagerNotifier>,
) -> SymbolManager {
    let config = create_symbol_manager_config(symbol_props);
    let mut symbol_manager = SymbolManager::with_config(config);
    symbol_manager.set_observer(Some(Arc::new(SamplySymbolManagerObserver::new(
        verbose, notifiers,
    ))));
    symbol_manager
}

/// Creates a [`SymbolManager`] together with the [`QuotaManager`] for its
/// symbol cache. For callers which only serve a single profile.
pub fn create_symbol_manager_and_quota_manager(
    symbol_props: SymbolProps,
    verbose: bool,
) -> (SymbolManager, Option<QuotaManager>) {
    let quota_manager = resolved_symbol_cache_dir(&symbol_props)
        .and_then(|dir| create_symbol_cache_quota_manager(&dir, &symbol_props.eviction));
    let notifiers = match &quota_manager {
        Some(mgr) => vec![mgr.notifier()],
        None => vec![],
    };
    if let Some(mgr) = &quota_manager {
        // Enforce the size limit and delete old files.
        mgr.notifier().trigger_eviction_if_needed();
    }
    let symbol_manager = create_symbol_manager(symbol_props, verbose, notifiers);
    (symbol_manager, quota_manager)
}
