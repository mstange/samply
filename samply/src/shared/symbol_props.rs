use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SymbolProps {
    /// Extra directories containing symbol files
    pub symbol_dir: Vec<PathBuf>,
    /// Additional URLs of symbol servers serving PDB / DLL / EXE files
    pub windows_symbol_server: Vec<String>,
    /// Overrides the default cache directory for Windows symbol files which were downloaded from a symbol server
    pub windows_symbol_cache: Option<PathBuf>,
    /// Additional URLs of symbol servers serving Breakpad .sym files
    pub breakpad_symbol_server: Vec<String>,
    /// Additional local directories containing Breakpad .sym files
    pub breakpad_symbol_dir: Vec<String>,
    /// Overrides the default cache directory for Breakpad symbol files
    pub breakpad_symbol_cache: Option<PathBuf>,
    /// Extra directory containing symbol files, with the directory structure used by simpleperf's scripts
    pub simpleperf_binary_cache: Option<PathBuf>,
}
