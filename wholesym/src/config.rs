use std::path::PathBuf;
use std::{collections::HashMap, sync::Arc};

use debugid::DebugId;
use symsrv::{parse_nt_symbol_path, NtSymbolPathEntry};

// Helper struct to avoid not being able to derive Debug on SymbolManagerConfig
pub(crate) struct PrecogDataContainer {
    pub(crate) precog_data: HashMap<DebugId, Arc<dyn samply_symbols::SymbolMapTrait + Send + Sync>>,
}

impl std::fmt::Debug for PrecogDataContainer {
    // Explicit implementation needed due to precog_data
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("...")
    }
}

/// The configuration of a [`SymbolManager`](crate::SymbolManager).
///
/// Allows specifying various sources of symbol files.
#[derive(Debug, Default)]
pub struct SymbolManagerConfig {
    pub(crate) verbose: bool,
    pub(crate) redirect_paths: HashMap<PathBuf, PathBuf>,
    pub(crate) respect_nt_symbol_path: bool,
    pub(crate) default_nt_symbol_path: Option<String>,
    pub(crate) breakpad_directories_readonly: Vec<PathBuf>,
    pub(crate) breakpad_servers: Vec<(String, PathBuf)>,
    pub(crate) breakpad_symindex_cache_dir: Option<PathBuf>,
    pub(crate) windows_servers: Vec<(String, PathBuf)>,
    pub(crate) use_debuginfod: bool,
    pub(crate) use_spotlight: bool,
    pub(crate) debuginfod_cache_dir_if_not_installed: Option<PathBuf>,
    pub(crate) debuginfod_servers: Vec<(String, PathBuf)>,
    pub(crate) precog_data: Option<PrecogDataContainer>,
}

impl SymbolManagerConfig {
    /// Create a new `SymbolManagerConfig` in its default state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Turns logging on or off.
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// For use in tests. Add a path which, when opened, opens a file at a different path instead.
    ///
    /// This can be used to test debug files which refer to other files on the file system with
    /// absolute paths, by redirecting those absolute paths to a path in the test fixtures directory.
    pub fn redirect_path_for_testing(
        mut self,
        redirect_path: impl Into<PathBuf>,
        dest_path: impl Into<PathBuf>,
    ) -> Self {
        self.redirect_paths
            .insert(redirect_path.into(), dest_path.into());
        self
    }

    /// Whether to import Windows symbol path configuration from the
    /// `_NT_SYMBOL_PATH` environment variable.
    pub fn respect_nt_symbol_path(mut self, respect: bool) -> Self {
        self.respect_nt_symbol_path = respect;
        self
    }

    /// Set a fallback value for the Windows symbol path which is used
    /// if `respect_nt_symbol_path` is false or if the `_NT_SYMBOL_PATH`
    /// environment variable is not set.
    ///
    /// Example: `"srv**https://msdl.microsoft.com/download/symbols"`
    pub fn default_nt_symbol_path(mut self, default_env_val: impl Into<String>) -> Self {
        self.default_nt_symbol_path = Some(default_env_val.into());
        self
    }

    pub(crate) fn effective_nt_symbol_path(&self) -> Option<Vec<NtSymbolPathEntry>> {
        let respected_env_value = if self.respect_nt_symbol_path {
            std::env::var("_NT_SYMBOL_PATH").ok()
        } else {
            None
        };
        let mut path = match (respected_env_value, &self.default_nt_symbol_path) {
            (Some(env_var), _) => Some(parse_nt_symbol_path(&env_var)),
            (None, Some(default)) => Some(parse_nt_symbol_path(default)),
            (None, None) => None,
        };
        for (base_url, cache_dir) in &self.windows_servers {
            path.get_or_insert_with(Default::default)
                .push(NtSymbolPathEntry::Chain {
                    dll: "symsrv.dll".to_string(),
                    cache_paths: vec![symsrv::CachePath::Path(cache_dir.clone())],
                    urls: vec![base_url.clone()],
                })
        }
        path
    }

    /// Add a directory to search for breakpad symbol files.
    ///
    /// The first-added directory will be searched first. Directories added here
    /// are only used for reading.
    pub fn breakpad_symbols_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.breakpad_directories_readonly.push(dir.into());
        self
    }

    /// Add a server to search for breakpad symbol files, along with a local cache directory.
    ///
    /// This method can be called multiple times; the servers and caches will be tried in the order of those calls.
    pub fn breakpad_symbols_server(
        mut self,
        base_url: impl Into<String>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        self.breakpad_servers
            .push((base_url.into(), cache_dir.into()));
        self
    }

    /// Set a directory to cache symindex files in. These files are created while
    /// downloading sym files from Breakpad symbol servers, and when opening existing
    /// sym files without a corresponding symindex file.
    ///
    /// Only one directory of this type can be set. This directory is used for both
    /// reading and writing.
    pub fn breakpad_symindex_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.breakpad_symindex_cache_dir = Some(dir.into());
        self
    }

    /// Add a server to search for Windows symbol files (pdb / exe / dll), along with a local cache directory.
    ///
    /// This method can be called multiple times; the servers and caches will be tried in the order of those calls.
    pub fn windows_symbols_server(
        mut self,
        base_url: impl Into<String>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        self.windows_servers
            .push((base_url.into(), cache_dir.into()));
        self
    }

    /// Whether debuginfod should be used, i.e. whether the `DEBUGINFOD_URLS` environment variable should be respected.
    ///
    /// At the moment this will only work if you specify a custom cache directory with `debuginfod_cache_dir_if_not_installed`.
    // TODO: Once "official debuginfod" is supported, change this to:
    // /// If debuginfod is not installed, this will only work if you specify a custom cache directory with `debuginfod_cache_dir_if_not_installed`.
    pub fn use_debuginfod(mut self, flag: bool) -> Self {
        self.use_debuginfod = flag;
        self
    }

    /// If `use_debuginfod` is set, use this directory as a cache directory. At the moment
    /// this is used even if debuginfod is installed, despite the function name, because
    /// wholesym is still missing a way to use the installed debuginfod and currently
    /// always runs its own code for downloading files from debuginfod servers.
    // TODO: Once "official debuginfod" is supported, change this to:
    // /// If `use_debuginfod` is set, and debuginfod is not installed (e.g. on non-Linux), use this directory as a cache directory.
    pub fn debuginfod_cache_dir_if_not_installed(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.debuginfod_cache_dir_if_not_installed = Some(cache_dir.into());
        self
    }

    /// Add a server to search for ELF debuginfo and executable files, along with a local cache directory.
    /// These servers are consulted independently of `use_debuginfod`.
    ///
    /// This method can be called multiple times; the servers and caches will be tried in the order of those calls.
    pub fn extra_debuginfod_server(
        mut self,
        base_url: impl Into<String>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        self.debuginfod_servers
            .push((base_url.into(), cache_dir.into()));
        self
    }

    /// Whether to use the macOS Spotlight service (`mdfind`) to look up the location
    /// of dSYM files based on a mach-O UUID. Ignored on non-macOS.
    pub fn use_spotlight(mut self, use_spotlight: bool) -> Self {
        self.use_spotlight = use_spotlight;
        self
    }

    /// Provide explicit symbol maps for a set of debug IDs.
    pub fn set_precog_data(
        mut self,
        precog_data: HashMap<DebugId, Arc<dyn samply_symbols::SymbolMapTrait + Send + Sync>>,
    ) -> Self {
        self.precog_data = Some(PrecogDataContainer { precog_data });
        self
    }
}
