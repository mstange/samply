use debugid::DebugId;
use samply_api::samply_symbols::{
    self, BreakpadIndex, BreakpadIndexParser, ElfBuildId, LibraryInfo, PeCodeId,
};
use samply_symbols::{
    CandidatePathInfo, CodeId, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    OptionallySendFuture,
};
use symsrv::{memmap2, FileContents, SymbolCache};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use std::{
    collections::HashMap,
    fs::{self, File},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

use crate::{config::SymbolManagerConfig, debuginfod::DebuginfodSymbolCache};

#[derive(Debug, Clone)]
pub enum WholesymFileLocation {
    LocalFile(PathBuf),
    SymsrvFile(String),
    LocalBreakpadFile(PathBuf, String),
    BreakpadSymbolServerFile(String),
    BreakpadSymindexFile(String),
    DebuginfodDebugFile(ElfBuildId),
    DebuginfodExecutable(ElfBuildId),
}

impl FileLocation for WholesymFileLocation {
    fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self> {
        // Dyld shared caches are only loaded from local files.
        match self {
            Self::LocalFile(cache_path) => {
                let mut filename = cache_path.file_name().unwrap().to_owned();
                filename.push(suffix);
                Some(Self::LocalFile(cache_path.with_file_name(filename)))
            }
            _ => None,
        }
    }

    fn location_for_external_object_file(&self, object_file: &str) -> Option<Self> {
        // External object files are referred to by absolute file path, so we only
        // load them if those paths were found in a local file.
        match self {
            Self::LocalFile(_) => Some(Self::LocalFile(object_file.into())),
            _ => None,
        }
    }

    fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self> {
        // We only respect absolute paths to PDB files if those paths were found in a local binary.
        match self {
            Self::LocalFile(_) => Some(Self::LocalFile(pdb_path_in_binary.into())),
            _ => None,
        }
    }

    fn location_for_source_file(&self, source_file_path: &str) -> Option<Self> {
        match self {
            Self::LocalFile(debug_file_path) => {
                let source_file_path = Path::new(source_file_path);
                if source_file_path.is_absolute() {
                    Some(Self::LocalFile(source_file_path.to_owned()))
                } else {
                    // Resolve relative paths with respect to the location of the debug file.
                    debug_file_path
                        .parent()
                        .map(|base_path| Self::LocalFile(base_path.join(source_file_path)))
                }
            }
            Self::DebuginfodDebugFile(_build_id) | Self::DebuginfodExecutable(_build_id) => {
                // TODO: load source file via debuginfod
                None
            }
            _ => {
                // We don't have local source files for debug files from symbol servers.
                // Ignore the absolute path in the downloaded file.
                None
            }
        }
    }

    fn location_for_breakpad_symindex(&self) -> Option<Self> {
        match self {
            Self::BreakpadSymbolServerFile(rel_path) | Self::LocalBreakpadFile(_, rel_path) => {
                Some(Self::BreakpadSymindexFile(rel_path.clone()))
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for WholesymFileLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{self:?}"))
    }
}

/// A simple helper which only exists to let samply_symbols::SymbolManager open
/// local binary files for the binary_at_path functions.
pub struct FileReadOnlyHelper;

impl FileReadOnlyHelper {
    async fn load_file_impl(
        &self,
        location: WholesymFileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            WholesymFileLocation::LocalFile(path) => {
                let file = File::open(path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            _ => {
                panic!("FileReadOnlyHelper should only be used for local files");
            }
        }
    }
}

impl<'h> FileAndPathHelper<'h> for FileReadOnlyHelper {
    type F = FileContents;
    type FL = WholesymFileLocation;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        _library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<WholesymFileLocation>>> {
        panic!("Should not be called");
    }

    fn get_candidate_paths_for_binary(
        &self,
        _library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<WholesymFileLocation>>> {
        panic!("Should not be called");
    }

    fn load_file(
        &'h self,
        location: WholesymFileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.load_file_impl(location))
    }

    fn get_dyld_shared_cache_paths(
        &self,
        arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<WholesymFileLocation>> {
        Ok(get_dyld_shared_cache_paths(arch))
    }
}

pub struct Helper {
    win_symbol_cache: Option<SymbolCache>,
    debuginfod_symbol_cache: Option<DebuginfodSymbolCache>,
    known_libs: Mutex<KnownLibs>,
    config: SymbolManagerConfig,
}

#[derive(Debug, Clone, Default)]
struct KnownLibs {
    by_debug: HashMap<(String, DebugId), Arc<LibraryInfo>>,
    by_pe: HashMap<(String, PeCodeId), Arc<LibraryInfo>>,
    by_elf_build_id: HashMap<ElfBuildId, Arc<LibraryInfo>>,
    by_mach_uuid: HashMap<Uuid, Arc<LibraryInfo>>,
}

impl Helper {
    pub fn with_config(config: SymbolManagerConfig) -> Self {
        let win_symbol_cache = match config.effective_nt_symbol_path() {
            Some(nt_symbol_path) => Some(SymbolCache::new(nt_symbol_path, config.verbose)),
            None => None,
        };
        let debuginfod_symbol_cache = if config.use_debuginfod {
            Some(DebuginfodSymbolCache::new(
                config.debuginfod_cache_dir_if_not_installed.clone(),
                config.debuginfod_servers.clone(),
                config.verbose,
            ))
        } else {
            None
        };
        Self {
            win_symbol_cache,
            debuginfod_symbol_cache,
            known_libs: Mutex::new(Default::default()),
            config,
        }
    }

    pub fn add_known_lib(&self, lib_info: LibraryInfo) {
        let mut known_libs = self.known_libs.lock().unwrap();
        let lib_info = Arc::new(lib_info);
        if let (Some(debug_name), Some(debug_id)) = (lib_info.debug_name.clone(), lib_info.debug_id)
        {
            known_libs
                .by_debug
                .insert((debug_name, debug_id), lib_info.clone());
        }
        match (lib_info.code_id.as_ref(), lib_info.name.as_deref()) {
            (Some(CodeId::PeCodeId(pe_code_id)), Some(name)) => {
                let pe_key = (name.to_string(), pe_code_id.clone());
                known_libs.by_pe.insert(pe_key, lib_info.clone());
            }
            (Some(CodeId::ElfBuildId(elf_build_id)), _) => {
                known_libs
                    .by_elf_build_id
                    .insert(elf_build_id.clone(), lib_info.clone());
            }
            (Some(CodeId::MachoUuid(uuid)), _) => {
                known_libs.by_mach_uuid.insert(*uuid, lib_info.clone());
            }
            _ => {}
        }
    }

    async fn load_file_impl(
        &self,
        location: WholesymFileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            WholesymFileLocation::LocalFile(path) => {
                if self.config.verbose {
                    eprintln!("Opening file {:?}", path.to_string_lossy());
                }
                let path = self.config.redirect_paths.get(&path).unwrap_or(&path);
                let file = File::open(path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            WholesymFileLocation::LocalBreakpadFile(path, rel_path) => {
                if self.config.verbose {
                    eprintln!("Opening file {:?}", path.to_string_lossy());
                }
                self.ensure_symindex(&path, &rel_path).await?;
                let file = File::open(path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            WholesymFileLocation::SymsrvFile(path) => {
                if self.config.verbose {
                    eprintln!("Trying to get file {path:?} from symbol cache");
                }
                Ok(self
                    .win_symbol_cache
                    .as_ref()
                    .unwrap()
                    .get_file(Path::new(&path))
                    .await?)
            }
            WholesymFileLocation::BreakpadSymbolServerFile(path) => {
                if self.config.verbose {
                    eprintln!("Trying to get file {path:?} from breakpad symbol server");
                }
                self.get_bp_sym_file(&path).await
            }
            WholesymFileLocation::BreakpadSymindexFile(rel_path) => {
                if let Some(symindex_path) = self.symindex_path(&rel_path) {
                    if self.config.verbose {
                        eprintln!("Opening file {:?}", symindex_path.to_string_lossy());
                    }
                    let file = File::open(symindex_path)?;
                    Ok(FileContents::Mmap(unsafe {
                        memmap2::MmapOptions::new().map(&file)?
                    }))
                } else {
                    Err("No breakpad symindex cache dir configured".into())
                }
            }
            WholesymFileLocation::DebuginfodDebugFile(build_id) => self
                .debuginfod_symbol_cache
                .as_ref()
                .unwrap()
                .get_file(&build_id.to_string(), "debuginfo")
                .await
                .ok_or_else(|| "Debuginfod could not find debuginfo".into()),
            WholesymFileLocation::DebuginfodExecutable(build_id) => self
                .debuginfod_symbol_cache
                .as_ref()
                .unwrap()
                .get_file(&build_id.to_string(), "debuginfo")
                .await
                .ok_or_else(|| "Debuginfod could not find debuginfo".into()),
        }
    }

    async fn get_bp_sym_file(&self, rel_path: &str) -> FileAndPathHelperResult<FileContents> {
        for (server_base_url, cache_dir) in &self.config.breakpad_servers {
            if let Ok(file) = self
                .get_bp_sym_file_from_server(rel_path, server_base_url, cache_dir)
                .await
            {
                return Ok(file);
            }
        }
        Err("No breakpad sym file on server".into())
    }

    async fn get_bp_sym_file_from_server(
        &self,
        rel_path: &str,
        server_base_url: &str,
        cache_dir: &Path,
    ) -> FileAndPathHelperResult<FileContents> {
        let url = format!("{server_base_url}/{rel_path}");
        if self.config.verbose {
            eprintln!("Downloading {url}...");
        }
        let sym_file_response = reqwest::get(&url).await?.error_for_status()?;
        let mut stream = sym_file_response.bytes_stream();
        let dest_path = cache_dir.join(rel_path);
        if let Some(dir) = dest_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        if self.config.verbose {
            eprintln!("Saving bytes to {dest_path:?}.");
        }
        let file = tokio::fs::File::create(&dest_path).await?;
        let mut writer = tokio::io::BufWriter::new(file);
        use futures_util::StreamExt;
        let mut parser = samply_symbols::BreakpadIndexParser::new();
        while let Some(item) = stream.next().await {
            let item = item?;
            let mut item_slice = item.as_ref();
            parser.consume(item_slice);
            tokio::io::copy(&mut item_slice, &mut writer).await?;
        }
        drop(writer);

        match parser.finish() {
            Ok(index) => self.write_symindex(rel_path, index).await?,
            Err(err) => {
                if self.config.verbose {
                    eprintln!("Breakpad parsing for symindex failed: {err}");
                }
            }
        }

        if self.config.verbose {
            eprintln!("Opening file {:?}", dest_path.to_string_lossy());
        }
        let file = File::open(&dest_path)?;
        Ok(FileContents::Mmap(unsafe {
            memmap2::MmapOptions::new().map(&file)?
        }))
    }

    fn symindex_path(&self, rel_path: &str) -> Option<PathBuf> {
        self.config
            .breakpad_symindex_cache_dir
            .as_deref()
            .map(|symindex_dir| symindex_dir.join(rel_path).with_extension("symindex"))
    }

    async fn write_symindex(
        &self,
        rel_path: &str,
        index: BreakpadIndex,
    ) -> FileAndPathHelperResult<()> {
        let symindex_path = self
            .symindex_path(rel_path)
            .ok_or("No breakpad symindex cache dir configured")?;
        if self.config.verbose {
            eprintln!("Writing symindex to {symindex_path:?}.");
        }
        let mut index_file = tokio::fs::File::create(&symindex_path).await?;
        index_file.write_all(&index.serialize_to_bytes()).await?;
        index_file.flush().await?;
        Ok(())
    }

    /// If we have a configured symindex cache directory, and there is a .sym file at
    /// `local_path` for which we don't have a .symindex file, create the .symindex file.
    async fn ensure_symindex(
        &self,
        local_dir: &Path,
        rel_path: &str,
    ) -> FileAndPathHelperResult<()> {
        if let Some(symindex_path) = self.symindex_path(rel_path) {
            if let (Ok(mut sym_file), Err(_symindex_file_error)) = (
                tokio::fs::File::open(local_dir).await,
                tokio::fs::File::open(symindex_path).await,
            ) {
                if self.config.verbose {
                    eprintln!("Found a Breakpad sym file at {local_dir:?} for which no symindex exists. Attempting to create symindex.");
                }
                let mut parser = BreakpadIndexParser::new();
                const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MiB
                let mut buffer = vec![0; CHUNK_SIZE];
                loop {
                    let read_len = sym_file.read(&mut buffer).await?;
                    if read_len == 0 {
                        break;
                    }
                    parser.consume(&buffer[..read_len]);
                }
                match parser.finish() {
                    Ok(index) => self.write_symindex(rel_path, index).await?,
                    Err(err) => {
                        if self.config.verbose {
                            eprintln!("Breakpad parsing for symindex failed: {err}");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn fill_in_library_info_details(&self, info: &mut LibraryInfo) {
        let known_libs = self.known_libs.lock().unwrap();

        // Look up (debugName, breakpadId) in the known libs.
        if let (Some(debug_name), Some(debug_id)) = (&info.debug_name, info.debug_id) {
            if let Some(known_info) = known_libs.by_debug.get(&(debug_name.to_string(), debug_id)) {
                info.absorb(known_info);
            }
        }

        // If all we have is the ELF build ID, maybe we have some paths in the known libs.
        let known_info = match (info.code_id.as_ref(), info.name.as_deref()) {
            (Some(CodeId::PeCodeId(pe_code_id)), Some(name)) => {
                let pe_key = (name.to_string(), pe_code_id.clone());
                known_libs.by_pe.get(&pe_key)
            }
            (Some(CodeId::ElfBuildId(elf_build_id)), _) => {
                known_libs.by_elf_build_id.get(elf_build_id)
            }
            (Some(CodeId::MachoUuid(uuid)), _) => known_libs.by_mach_uuid.get(uuid),
            _ => None,
        };
        if let Some(known_info) = known_info {
            info.absorb(known_info);
        }
    }
}

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContents;
    type FL = WholesymFileLocation;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<WholesymFileLocation>>> {
        let mut paths = vec![];

        let mut info = library_info.clone();
        self.fill_in_library_info_details(&mut info);

        let mut got_dsym = false;

        if let (Some(debug_path), Some(debug_name)) = (&info.debug_path, &info.debug_name) {
            if let Some(debug_id) = info.debug_id {
                // First, see if we can find a dSYM file for the binary.
                if let Some(dsym_path) =
                    crate::moria_mac::locate_dsym_fastpath(Path::new(debug_path), debug_id.uuid())
                {
                    got_dsym = true;
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::LocalFile(dsym_path.clone()),
                    ));
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::LocalFile(
                            dsym_path
                                .join("Contents")
                                .join("Resources")
                                .join("DWARF")
                                .join(debug_name),
                        ),
                    ));
                }
            }

            // Also consider .so.dbg files in the same directory.
            if debug_path.ends_with(".so") {
                let so_dbg_path = format!("{debug_path}.dbg");
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(PathBuf::from(so_dbg_path)),
                ));
            }

            if debug_path.ends_with(".pdb") {
                // Get symbols from the pdb file.
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(debug_path.into()),
                ));
            }
        }

        if !got_dsym && self.config.use_spotlight {
            if let Some(debug_id) = info.debug_id {
                // Try a little harder to find a dSYM, just from the UUID. We can do this
                // even if we don't have an entry for this library in the libinfo map.
                if let Ok(dsym_path) =
                    crate::moria_mac::locate_dsym_using_spotlight(debug_id.uuid())
                {
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::LocalFile(dsym_path.clone()),
                    ));
                    if let Some(dsym_file_name) = dsym_path.file_name().and_then(|s| s.to_str()) {
                        paths.push(CandidatePathInfo::SingleFile(
                            WholesymFileLocation::LocalFile(
                                dsym_path
                                    .join("Contents")
                                    .join("Resources")
                                    .join("DWARF")
                                    .join(dsym_file_name.trim_end_matches(".dSYM")),
                            ),
                        ));
                    }
                }
            }
        }

        // Find debuginfo in /usr/lib/debug/.build-id/ etc.
        // <https://sourceware.org/gdb/onlinedocs/gdb/Separate-Debug-Files.html>
        if let Some(CodeId::ElfBuildId(build_id)) = &info.code_id {
            let build_id = build_id.to_string();
            if build_id.len() > 2 {
                let (two_chars, rest) = build_id.split_at(2);
                let path = format!("/usr/lib/debug/.build-id/{two_chars}/{rest}.debug");
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(PathBuf::from(path)),
                ));
            }
        }

        if let (Some(debug_name), Some(debug_id)) = (&info.debug_name, info.debug_id) {
            let rel_path = format!(
                "{}/{}/{}.sym",
                debug_name,
                debug_id.breakpad(),
                debug_name.trim_end_matches(".pdb")
            );

            // Search breakpad symbol directories.
            for dir in &self.config.breakpad_directories_readonly {
                let local_path = dir.join(&rel_path);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalBreakpadFile(local_path, rel_path.clone()),
                ));
            }

            for (_url, dir) in &self.config.breakpad_servers {
                let local_path = dir.join(&rel_path);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalBreakpadFile(local_path, rel_path.clone()),
                ));
            }

            // TODO: Check symsrv local cache before checking breakpad servers
            // but still check breakpad server before checking symsrv server

            if !self.config.breakpad_servers.is_empty() {
                // We might find a .sym file on a symbol server.
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::BreakpadSymbolServerFile(rel_path),
                ));
            }

            if debug_name.ends_with(".pdb") && self.win_symbol_cache.is_some() {
                // We might find this pdb file with the help of a symbol server.
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::SymsrvFile(format!(
                        "{}/{}/{}",
                        debug_name,
                        debug_id.breakpad(),
                        debug_name
                    )),
                ));
            }
        }

        if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
            (self.debuginfod_symbol_cache.as_ref(), &info.code_id)
        {
            if !might_be_perf_jit_so_file(&info) {
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::DebuginfodDebugFile(build_id.to_owned()),
                ));
            }
        }

        if let Some(path) = &info.path {
            // Fall back to getting symbols from the binary itself.
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::LocalFile(path.into()),
            ));

            // For macOS system libraries, also consult the dyld shared cache.
            if path.starts_with("/usr/") || path.starts_with("/System/") {
                for dyld_cache_path in get_dyld_shared_cache_paths(info.arch.as_deref()) {
                    paths.push(CandidatePathInfo::InDyldCache {
                        dyld_cache_path,
                        dylib_path: path.clone(),
                    });
                }
            }
        }

        Ok(paths)
    }

    fn get_candidate_paths_for_gnu_debug_link_dest(
        &self,
        original_file_location: &WholesymFileLocation,
        debug_link_name: &str,
    ) -> FileAndPathHelperResult<Vec<WholesymFileLocation>> {
        let absolute_original_file_parent = match original_file_location {
            WholesymFileLocation::LocalFile(path) => {
                let parent = path
                    .parent()
                    .ok_or("Original file should point to a file")?;
                fs::canonicalize(parent)?
            }
            _ => return Err("Only local files have a .gnu_debuglink".into()),
        };

        // https://www-zeuthen.desy.de/unix/unixguide/infohtml/gdb/Separate-Debug-Files.html
        Ok(vec![
            WholesymFileLocation::LocalFile(absolute_original_file_parent.join(debug_link_name)),
            WholesymFileLocation::LocalFile(
                absolute_original_file_parent
                    .join(".debug")
                    .join(debug_link_name),
            ),
            WholesymFileLocation::LocalFile(
                Path::new("/usr/lib/debug")
                    .join(absolute_original_file_parent.strip_prefix("/")?)
                    .join(debug_link_name),
            ),
        ])
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<WholesymFileLocation>>> {
        let mut info = library_info.clone();
        self.fill_in_library_info_details(&mut info);

        let mut paths = vec![];

        // Begin with the binary itself.
        if let Some(path) = &info.path {
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::LocalFile(path.into()),
            ));
        }

        if let (Some(_symbol_cache), Some(name), Some(CodeId::PeCodeId(code_id))) =
            (&self.win_symbol_cache, &info.name, &info.code_id)
        {
            // We might find this exe / dll file with the help of a symbol server.
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::SymsrvFile(format!("{name}/{code_id}/{name}")),
            ));
        }

        if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
            (self.debuginfod_symbol_cache.as_ref(), &info.code_id)
        {
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::DebuginfodExecutable(build_id.to_owned()),
            ));
        }

        if let Some(path) = &info.path {
            // For macOS system libraries, also consult the dyld shared cache.
            if path.starts_with("/usr/") || path.starts_with("/System/") {
                for dyld_cache_path in get_dyld_shared_cache_paths(info.arch.as_deref()) {
                    paths.push(CandidatePathInfo::InDyldCache {
                        dyld_cache_path,
                        dylib_path: path.clone(),
                    });
                }
            }
        }

        Ok(paths)
    }

    fn get_dyld_shared_cache_paths(
        &self,
        arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<WholesymFileLocation>> {
        Ok(get_dyld_shared_cache_paths(arch))
    }

    fn load_file(
        &'h self,
        location: WholesymFileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.load_file_impl(location))
    }

    fn get_candidate_paths_for_supplementary_debug_file(
        &self,
        original_file_path: &WholesymFileLocation,
        sup_file_path: &str,
        sup_file_build_id: &ElfBuildId,
    ) -> FileAndPathHelperResult<Vec<WholesymFileLocation>> {
        let mut paths = Vec::new();

        if let WholesymFileLocation::LocalFile(original_file_path) = original_file_path {
            if sup_file_path.starts_with('/') {
                paths.push(WholesymFileLocation::LocalFile(PathBuf::from(
                    sup_file_path,
                )));
            } else if let Some(parent_dir) = original_file_path.parent() {
                let sup_file_path = parent_dir.join(Path::new(sup_file_path));
                paths.push(WholesymFileLocation::LocalFile(sup_file_path));
            }
        } else {
            // If the original debug file was non-local, don't check local files for
            // the supplementary debug file. The supplementary paths which are stored
            // in the downloaded file will usually refer to a path on the original build
            // machine, not on this machine.
        }

        let build_id = sup_file_build_id.to_string();
        if build_id.len() > 2 {
            let (two_chars, rest) = build_id.split_at(2);
            let path = format!("/usr/lib/debug/.build-id/{two_chars}/{rest}.debug");
            paths.push(WholesymFileLocation::LocalFile(PathBuf::from(path)));

            if self.debuginfod_symbol_cache.is_some() {
                paths.push(WholesymFileLocation::DebuginfodDebugFile(
                    sup_file_build_id.to_owned(),
                ));
            }
        }

        Ok(paths)
    }
}

fn get_dyld_shared_cache_paths(arch: Option<&str>) -> Vec<WholesymFileLocation> {
    if let Some(arch) = arch {
        vec![
            WholesymFileLocation::LocalFile(
                format!(
                "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_{arch}"
            )
                .into(),
            ),
            WholesymFileLocation::LocalFile(
                format!("/System/Library/dyld/dyld_shared_cache_{arch}").into(),
            ),
        ]
    } else {
        vec![
            WholesymFileLocation::LocalFile(
                "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e"
                    .into(),
            ),
            WholesymFileLocation::LocalFile(
                "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_x86_64"
                    .into(),
            ),
            WholesymFileLocation::LocalFile("/System/Library/dyld/dyld_shared_cache_arm64e".into()),
            WholesymFileLocation::LocalFile(
                "/System/Library/dyld/dyld_shared_cache_x86_64h".into(),
            ),
            WholesymFileLocation::LocalFile("/System/Library/dyld/dyld_shared_cache_x86_64".into()),
        ]
    }
}

/// Used to filter out files like `jitted-12345-12.so`, to avoid hammering debuginfod servers.
fn might_be_perf_jit_so_file(info: &LibraryInfo) -> bool {
    matches!(&info.name, Some(name) if name.starts_with("jitted-") && name.ends_with(".so"))
}
