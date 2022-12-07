use debugid::{CodeId, DebugId};
use std::collections::HashMap;
use symsrv::NtSymbolPathEntry;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibraryInfo {
    pub debug_name: String,
    pub debug_id: DebugId,
    pub debug_path: Option<String>,
    pub path: Option<String>,
    pub code_id: Option<CodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolManagerConfig {
    pub(crate) known_libs: HashMap<(String, DebugId), LibraryInfo>,
    pub(crate) verbose: bool,
    pub(crate) nt_symbol_path: Option<Vec<NtSymbolPathEntry>>,
}

impl SymbolManagerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn with_known_lib(mut self, lib_info: LibraryInfo) -> Self {
        self.known_libs
            .insert((lib_info.debug_name.clone(), lib_info.debug_id), lib_info);
        self
    }

    pub fn with_nt_symbol_path(mut self, nt_symbol_path: Vec<NtSymbolPathEntry>) -> Self {
        self.nt_symbol_path = Some(nt_symbol_path);
        self
    }
}
