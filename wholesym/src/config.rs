use std::{collections::HashMap, path::PathBuf};

use symsrv::{parse_nt_symbol_path, NtSymbolPathEntry};

#[derive(Debug, Clone, Default)]
pub struct SymbolManagerConfig {
    pub(crate) verbose: bool,
    pub(crate) redirect_paths: HashMap<PathBuf, PathBuf>,
    pub(crate) respect_nt_symbol_path: bool,
    pub(crate) default_nt_symbol_path: Option<String>,
    pub(crate) breakpad_directories_readonly: Vec<PathBuf>,
    pub(crate) breakpad_servers: Vec<(String, PathBuf)>,
    pub(crate) windows_servers: Vec<(String, PathBuf)>,
}

impl SymbolManagerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// For use in tests. Add a path which, when opened, opens a file at a different path instead.
    ///
    /// This can be used to test debug files which refer to other files on the file system with
    /// absolute paths, by redirecting those absolute paths to a path in the test fixtures directory.
    pub fn redirect_path_for_testing(mut self, redirect_path: PathBuf, dest_path: PathBuf) -> Self {
        self.redirect_paths.insert(redirect_path, dest_path);
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
        let default_downstream_store = symsrv::get_default_downstream_store();
        let default_downstream_store = default_downstream_store.as_deref();
        let respected_env_value = if self.respect_nt_symbol_path {
            std::env::var("_NT_SYMBOL_PATH").ok()
        } else {
            None
        };
        let mut path = match (respected_env_value, &self.default_nt_symbol_path) {
            (Some(env_var), _) => Some(parse_nt_symbol_path(&env_var, default_downstream_store)),
            (None, Some(default)) => Some(parse_nt_symbol_path(default, default_downstream_store)),
            (None, None) => None,
        };
        for (base_url, cache_dir) in &self.windows_servers {
            path.get_or_insert_with(Default::default)
                .push(NtSymbolPathEntry::Chain {
                    dll: "symsrv.dll".to_string(),
                    cache_paths: vec![cache_dir.clone()],
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
}
