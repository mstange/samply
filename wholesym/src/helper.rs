use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use debugid::DebugId;
use samply_symbols::{
    CandidatePathInfo, CodeId, ElfBuildId, FileAndPathHelper, FileAndPathHelperResult,
    FileLocation, LibraryInfo, OptionallySendFuture, PeCodeId, SymbolMapTrait,
};
use symsrv::{SymsrvDownloader, SymsrvObserver};
use uuid::Uuid;

use crate::breakpad::BreakpadSymbolDownloader;
use crate::config::SymbolManagerConfig;
use crate::debuginfod::DebuginfodDownloader;
use crate::downloader::{Downloader, DownloaderObserver};
use crate::vdso::get_vdso_data;
use crate::{DownloadError, SymbolManagerObserver};

/// This is how the symbol file contents are returned. If there's an uncompressed file
/// in the store, then we return an Mmap of that uncompressed file. If there is no
/// local file or the local file is compressed, then we load or uncompress the file
/// into memory and return a `Bytes` wrapper of that memory.
///
/// This type can be coerced to a [u8] slice with `&file_contents[..]`.
pub enum WholesymFileContents {
    /// A mapped file.
    Mmap(memmap2::Mmap),
    /// Bytes in memory.
    Bytes(Bytes),
}

impl std::ops::Deref for WholesymFileContents {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            WholesymFileContents::Mmap(mmap) => mmap,
            WholesymFileContents::Bytes(bytes) => bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WholesymFileLocation {
    LocalFile(PathBuf),
    LocalSymsrvFile(String, String),
    LocalBreakpadFile(String),
    SymsrvFile(String, String),
    BreakpadSymbolServerFile(String),
    BreakpadSymindexFile(String),
    DebuginfodDebugFile(ElfBuildId),
    DebuginfodExecutable(ElfBuildId),
    UrlForSourceFile(String),
    VdsoLoadedIntoThisProcess,
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
                if source_file_path.starts_with("https://")
                    || source_file_path.starts_with("http://")
                {
                    // Treat the path as a URL. One case where we get URLs is in jitdump files:
                    // E.g. profiling a browser which executes JITted JS code from a script on
                    // the web will create a jitdump file where the debug information for an
                    // address has a URL as the file path.
                    //
                    // SECURITY: This URL is referred to by a debug file on the local file system.
                    // We trust the contents of these files, and we allow them to refer to
                    // arbitrary URLs.
                    return Some(Self::UrlForSourceFile(source_file_path.to_owned()));
                }
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
            Self::BreakpadSymbolServerFile(rel_path) | Self::LocalBreakpadFile(rel_path) => {
                Some(Self::BreakpadSymindexFile(rel_path.clone()))
            }
            _ => None,
        }
    }

    fn location_for_dwo(&self, comp_dir: &str, path: &str) -> Option<Self> {
        // Dwo files are referred to by absolute file path, so we only
        // load them if those paths were found in a local file.
        match self {
            Self::LocalFile(debug_file_path) => {
                if path.starts_with('/') {
                    return Some(Self::LocalFile(path.into()));
                }
                // Resolve relative paths with respect to comp_dir.
                if comp_dir.starts_with('/') {
                    let comp_dir = comp_dir.trim_end_matches('/');
                    let dwo_path = format!("{comp_dir}/{path}");
                    return Some(Self::LocalFile(Path::new(&dwo_path).into()));
                }
                // Resolve relative paths with respect to the location of the debug file.
                debug_file_path
                    .parent()
                    .map(|base_path| Self::LocalFile(base_path.join(comp_dir).join(path)))
            }
            _ => None,
        }
    }

    fn location_for_dwp(&self) -> Option<Self> {
        // DWP files are only used locally; by convention they are named
        // "<binaryname>.dwp" and placed next to the corresponding binary.
        // The original binary does not have a pointer to the DWP file.
        // DWP files also do not have a build ID, they cannot be looked up
        // from a symbol server. The debug information inside a DWP file is
        // only useful in combination with the debug info inside the binary
        // (the "skeleton units"); a DWP file by itself cannot be used to
        // look up symbols if the binary has been stripped of debug info.
        match self {
            Self::LocalFile(binary_path) => {
                let mut dwp_path = binary_path.as_os_str().to_os_string();
                dwp_path.push(".dwp");
                Some(Self::LocalFile(dwp_path.into()))
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
    ) -> FileAndPathHelperResult<WholesymFileContents> {
        match location {
            WholesymFileLocation::LocalFile(path) => {
                let file = File::open(path)?;
                Ok(WholesymFileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            _ => {
                panic!("FileReadOnlyHelper should only be used for local files");
            }
        }
    }
}

impl FileAndPathHelper for FileReadOnlyHelper {
    type F = WholesymFileContents;
    type FL = WholesymFileLocation;

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
        &self,
        location: WholesymFileLocation,
    ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + '_>>
    {
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
    downloader: Arc<Downloader>,
    symsrv_downloader: Option<SymsrvDownloader>,
    breakpad_downloader: BreakpadSymbolDownloader,
    debuginfod_downloader: Option<DebuginfodDownloader>,
    known_libs: Mutex<KnownLibs>,
    config: SymbolManagerConfig,
    precog_symbol_data: Mutex<HashMap<DebugId, Arc<dyn SymbolMapTrait + Send + Sync>>>,
    observer: Arc<HelperDownloaderObserver>,
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
        let observer = Arc::new(HelperDownloaderObserver::new());
        let downloader = Arc::new(Downloader::new());
        let symsrv_downloader = match config.effective_nt_symbol_path() {
            Some(nt_symbol_path) => {
                let mut downloader = SymsrvDownloader::new(nt_symbol_path);
                downloader.set_default_downstream_store(symsrv::get_home_sym_dir());
                downloader.set_observer(Some(observer.clone()));
                Some(downloader)
            }
            None => None,
        };
        let debuginfod_downloader = if config.use_debuginfod {
            let mut downloader = DebuginfodDownloader::new(
                config.debuginfod_cache_dir_if_not_installed.clone(),
                config.debuginfod_servers.clone(),
                Some(downloader.clone()),
            );
            downloader.set_observer(Some(observer.clone()));
            Some(downloader)
        } else {
            None
        };
        let mut breakpad_downloader = BreakpadSymbolDownloader::new(
            config.breakpad_directories_readonly.clone(),
            config.breakpad_servers.clone(),
            config.breakpad_symindex_cache_dir.clone(),
            Some(downloader.clone()),
        );
        breakpad_downloader.set_observer(Some(observer.clone()));
        Self {
            downloader,
            symsrv_downloader,
            breakpad_downloader,
            debuginfod_downloader,
            known_libs: Mutex::new(Default::default()),
            config,
            precog_symbol_data: Mutex::new(Default::default()),
            observer,
        }
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn SymbolManagerObserver>>) {
        self.observer.set_observer(observer);
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

    pub fn add_precog_symbol_map(
        &self,
        lib_info: LibraryInfo,
        symbol_map: Arc<dyn SymbolMapTrait + Send + Sync>,
    ) {
        let debug_id = lib_info
            .debug_id
            .expect("LibraryInfo must have a debug_id to add precog symbols");
        let mut precog_symbol_data = self.precog_symbol_data.lock().unwrap();
        precog_symbol_data.insert(debug_id, symbol_map);
    }

    /// Return whether a file is found at `path`, and notify the observer if not.
    async fn check_file_exists(&self, path: &Path) -> bool {
        let file_exists = matches!(tokio::fs::metadata(path).await, Ok(meta) if meta.is_file());
        if !file_exists {
            self.observer.on_file_missed(path);
        }
        file_exists
    }

    async fn load_file_impl(
        &self,
        location: WholesymFileLocation,
    ) -> FileAndPathHelperResult<WholesymFileContents> {
        let file_path = match location {
            WholesymFileLocation::LocalFile(path) => {
                let path = self.config.redirect_paths.get(&path).unwrap_or(&path);
                if !self.check_file_exists(path).await {
                    return Err(format!("File not found: {path:?}").into());
                }
                path.to_owned()
            }
            WholesymFileLocation::LocalSymsrvFile(filename, hash) => {
                self.symsrv_downloader
                    .as_ref()
                    .unwrap()
                    .get_file_no_download(&filename, &hash)
                    .await?
            }
            WholesymFileLocation::LocalBreakpadFile(rel_path) => self
                .breakpad_downloader
                .get_file_no_download(&rel_path)
                .await
                .ok_or("Not found on breakpad symbol server")?,
            WholesymFileLocation::UrlForSourceFile(url) => {
                let download = self
                    .downloader
                    .initiate_download(&url, Some(self.observer.clone()))
                    .await?;
                let bytes = download.download_to_memory(None).await?;
                return Ok(WholesymFileContents::Bytes(bytes.into()));
            }
            WholesymFileLocation::SymsrvFile(filename, hash) => {
                self.symsrv_downloader
                    .as_ref()
                    .unwrap()
                    .get_file(&filename, &hash)
                    .await?
            }
            WholesymFileLocation::BreakpadSymbolServerFile(path) => self
                .breakpad_downloader
                .get_file(&path)
                .await
                .ok_or("Not found on breakpad symbol server")?,
            WholesymFileLocation::BreakpadSymindexFile(rel_path) => {
                let sym_path = self
                    .breakpad_downloader
                    .get_file_no_download(&rel_path)
                    .await
                    .ok_or("Not found in breakpad symbol directories")?;
                self.breakpad_downloader
                    .ensure_symindex(&sym_path, &rel_path)
                    .await?
            }
            WholesymFileLocation::DebuginfodDebugFile(build_id) => self
                .debuginfod_downloader
                .as_ref()
                .unwrap()
                .get_file(&build_id.to_string(), "debuginfo")
                .await
                .ok_or("Debuginfod could not find debuginfo")?,
            WholesymFileLocation::DebuginfodExecutable(build_id) => self
                .debuginfod_downloader
                .as_ref()
                .unwrap()
                .get_file(&build_id.to_string(), "executable")
                .await
                .ok_or("Debuginfod could not find executable")?,
            WholesymFileLocation::VdsoLoadedIntoThisProcess => {
                let vdso = get_vdso_data().ok_or("No vdso in this process")?;
                // Pretend that the VDSO data came from a file.
                // This works more or less by accident; object's parsing is made for
                // objects stored on disk, not for objects loaded into memory.
                // However, the VDSO in-memory image happens to be similar enough to its
                // equivalent on-disk image that this works fine. Most importantly, the
                // VDSO's section SVMAs match the section file offsets.
                return Ok(WholesymFileContents::Bytes(Bytes::copy_from_slice(vdso)));
            }
        };

        self.observer.on_file_accessed(&file_path);
        Ok(WholesymFileContents::Mmap(unsafe {
            memmap2::MmapOptions::new().map(&File::open(file_path)?)?
        }))
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

impl FileAndPathHelper for Helper {
    type F = WholesymFileContents;
    type FL = WholesymFileLocation;

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

        if let (Some(path), Some(debug_name)) = (&info.path, &info.debug_name) {
            if info.name.as_deref() != Some(debug_name) {
                // Also look for the debug file right next to the binary.
                let binary_path = Path::new(path);
                if let Some(parent) = binary_path.parent() {
                    let debug_path = parent.join(debug_name);
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::LocalFile(debug_path),
                    ));
                }
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
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::LocalBreakpadFile(rel_path.clone()),
            ));

            if debug_name.ends_with(".pdb") && self.symsrv_downloader.is_some() {
                // We might find this pdb file with the help of a symbol server.
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalSymsrvFile(
                        debug_name.clone(),
                        debug_id.breakpad().to_string(),
                    ),
                ));
            }

            if !might_be_fake_jit_file(&info) {
                if !self.config.breakpad_servers.is_empty() {
                    // We might find a .sym file on a symbol server.
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::BreakpadSymbolServerFile(rel_path),
                    ));
                }

                if debug_name.ends_with(".pdb") && self.symsrv_downloader.is_some() {
                    // We might find this pdb file with the help of a symbol server.
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::SymsrvFile(
                            debug_name.clone(),
                            debug_id.breakpad().to_string(),
                        ),
                    ));
                }
            }
        }

        if let Some(debug_name) = &info.debug_name {
            for symbol_dir in &self.config.extra_symbol_directories {
                let p = symbol_dir.join(debug_name);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(p),
                ));
            }
        }

        if !might_be_fake_jit_file(&info) {
            if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
                (self.debuginfod_downloader.as_ref(), &info.code_id)
            {
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::DebuginfodDebugFile(build_id.to_owned()),
                ));
            }
        }

        // Check any simpleperf binary_cache directories.
        if let (Some(binary_name), Some(CodeId::ElfBuildId(build_id))) = (&info.name, &info.code_id)
        {
            // Only do this for .so files for now. We don't properly support .oat / .vdex / .jar / .apk
            // and don't want to overwrite any existing better symbols.
            if binary_name.ends_with(".so") {
                // Example: binary_cache/5e5c7b9cbc3e65b7c98a139fc1d3e0d000000000-libadreno_utils.so
                let mut build_id_20 = build_id.0.clone();
                build_id_20.resize(20, 0);
                let name_in_cache =
                    format!("{}-{}", ElfBuildId::from_bytes(&build_id_20), binary_name);
                for dir in &self.config.simpleperf_binary_cache_directories {
                    let p = dir.join(&name_in_cache);
                    paths.push(CandidatePathInfo::SingleFile(
                        WholesymFileLocation::LocalFile(p),
                    ));
                }
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

        // Also look for the binary in the extra symbol directories.
        if let Some(name) = &info.name {
            for symbol_dir in &self.config.extra_symbol_directories {
                let p = symbol_dir.join(name);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(p),
                ));
            }
        }

        if info.name.as_deref() == Some("[vdso]") {
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::VdsoLoadedIntoThisProcess,
            ));
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
        let mut candidates = vec![
            WholesymFileLocation::LocalFile(absolute_original_file_parent.join(debug_link_name)),
            WholesymFileLocation::LocalFile(
                absolute_original_file_parent
                    .join(".debug")
                    .join(debug_link_name),
            ),
        ];
        if let Ok(relative_bin_path) = absolute_original_file_parent.strip_prefix("/") {
            candidates.push(WholesymFileLocation::LocalFile(
                Path::new("/usr/lib/debug")
                    .join(relative_bin_path)
                    .join(debug_link_name),
            ));
        }
        Ok(candidates)
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

        // Also look for the binary in the extra symbol directories.
        if let Some(name) = &info.name {
            for symbol_dir in &self.config.extra_symbol_directories {
                let p = symbol_dir.join(name);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(p),
                ));
            }
        }

        // Check any simpleperf binary_cache directories.
        if let (Some(binary_name), Some(CodeId::ElfBuildId(build_id))) = (&info.name, &info.code_id)
        {
            // Example: binary_cache/5e5c7b9cbc3e65b7c98a139fc1d3e0d000000000-libadreno_utils.so
            let mut build_id_20 = build_id.0.clone();
            build_id_20.resize(20, 0);
            let name_in_cache = format!("{}-{}", ElfBuildId::from_bytes(&build_id_20), binary_name);
            for dir in &self.config.simpleperf_binary_cache_directories {
                let p = dir.join(&name_in_cache);
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::LocalFile(p),
                ));
            }
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

        if info.name.as_deref() == Some("[vdso]") {
            paths.push(CandidatePathInfo::SingleFile(
                WholesymFileLocation::VdsoLoadedIntoThisProcess,
            ));
        }

        if !might_be_fake_jit_file(&info) {
            if let (Some(_symbol_cache), Some(name), Some(CodeId::PeCodeId(code_id))) =
                (&self.symsrv_downloader, &info.name, &info.code_id)
            {
                // We might find this exe / dll file with the help of a symbol server.
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::SymsrvFile(name.clone(), code_id.to_string()),
                ));
            }

            if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
                (self.debuginfod_downloader.as_ref(), &info.code_id)
            {
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::DebuginfodExecutable(build_id.to_owned()),
                ));
                paths.push(CandidatePathInfo::SingleFile(
                    WholesymFileLocation::DebuginfodDebugFile(build_id.to_owned()),
                ));
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
        &self,
        location: WholesymFileLocation,
    ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + '_>>
    {
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

            if self.debuginfod_downloader.is_some() {
                paths.push(WholesymFileLocation::DebuginfodDebugFile(
                    sup_file_build_id.to_owned(),
                ));
            }
        }

        Ok(paths)
    }

    fn get_symbol_map_for_library(
        &self,
        info: &LibraryInfo,
    ) -> Option<(Self::FL, Arc<dyn SymbolMapTrait + Send + Sync>)> {
        let precog_symbol_data = self.precog_symbol_data.lock().unwrap();
        let symbol_map = precog_symbol_data.get(&info.debug_id?)?;
        let location = WholesymFileLocation::LocalFile(
            info.debug_path
                .clone()
                .unwrap_or_else(|| "UNKNOWN".to_string())
                .into(),
        );
        Some((location, symbol_map.clone()))
    }
}

/// Return a Vec containing the potential paths where a dyld shared cache
/// which contains an object of the given architecture might be found.
///
/// For example, the architecture might have been derived from the mach-O
/// header of an object that was found in memory (e.g. the dyld images list
/// of a profiled process).
fn get_dyld_shared_cache_paths(arch: Option<&str>) -> Vec<WholesymFileLocation> {
    let mut vec = Vec::new();

    let mut add_entries_in_dir = |dir: &str| {
        let mut add_entry_for_arch = |arch: &str| {
            let path = format!("{dir}/dyld_shared_cache_{arch}");
            vec.push(WholesymFileLocation::LocalFile(PathBuf::from(path)));
        };
        match arch {
            None => {
                // Try all known architectures.
                add_entry_for_arch("arm64e");
                add_entry_for_arch("x86_64h");
                add_entry_for_arch("x86_64");
            }
            Some("x86_64") => {
                // x86_64 binaries can be either in the x86_64 or in the x86_64h cache.
                add_entry_for_arch("x86_64h");
                add_entry_for_arch("x86_64");
            }
            Some(arch) => {
                // Use the cache that matches the CPU architecture of the object file.
                add_entry_for_arch(arch);
            }
        }
    };

    // macOS 13+:
    add_entries_in_dir("/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld");
    // macOS 11 until macOS 13:
    add_entries_in_dir("/System/Library/dyld");

    vec
}

/// Used to filter out files like `jitted-12345-12.so`, to avoid hammering debuginfod servers.
fn might_be_fake_jit_file(info: &LibraryInfo) -> bool {
    matches!(&info.name, Some(name) if (name.starts_with("jitted-") && name.ends_with(".so")) || name.contains("jit_app_cache:"))
}

struct HelperDownloaderObserver {
    inner: Mutex<HelperDownloaderObserverInner>,
}

struct HelperDownloaderObserverInner {
    observer: Option<Arc<dyn SymbolManagerObserver>>,
    symsrv_download_id_mapping: HashMap<u64, u64>,
    downloader_download_id_mapping: HashMap<u64, u64>,
}

impl HelperDownloaderObserver {
    pub fn new() -> Self {
        let inner = HelperDownloaderObserverInner {
            observer: None,
            symsrv_download_id_mapping: HashMap::new(),
            downloader_download_id_mapping: HashMap::new(),
        };
        Self {
            inner: Mutex::new(inner),
        }
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn SymbolManagerObserver>>) {
        let mut inner = self.inner.lock().unwrap();
        inner.observer = observer;
    }

    pub fn on_file_accessed(&self, path: &Path) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_file_accessed(path);
    }

    pub fn on_file_missed(&self, path: &Path) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_file_missed(path);
    }
}

static NEXT_DOWNLOAD_ID: AtomicU64 = AtomicU64::new(0);

impl SymsrvObserver for HelperDownloaderObserver {
    fn on_new_download_before_connect(&self, symsrv_download_id: u64, url: &str) {
        let download_id = NEXT_DOWNLOAD_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        inner
            .symsrv_download_id_mapping
            .insert(symsrv_download_id, download_id);
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_new_download_before_connect(download_id, url);
    }

    fn on_download_started(&self, symsrv_download_id: u64) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        let download_id = inner.symsrv_download_id_mapping[&symsrv_download_id];
        drop(inner);
        observer.on_download_started(download_id);
    }

    fn on_download_progress(
        &self,
        symsrv_download_id: u64,
        bytes_so_far: u64,
        total_bytes: Option<u64>,
    ) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        let download_id = inner.symsrv_download_id_mapping[&symsrv_download_id];
        drop(inner);
        observer.on_download_progress(download_id, bytes_so_far, total_bytes);
    }

    fn on_download_completed(
        &self,
        symsrv_download_id: u64,
        uncompressed_size_in_bytes: u64,
        time_until_headers: std::time::Duration,
        time_until_completed: std::time::Duration,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .symsrv_download_id_mapping
            .remove(&symsrv_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_download_completed(
            download_id,
            uncompressed_size_in_bytes,
            time_until_headers,
            time_until_completed,
        );
    }

    fn on_download_failed(&self, symsrv_download_id: u64, reason: symsrv::DownloadError) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .symsrv_download_id_mapping
            .remove(&symsrv_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        let err = match reason {
            symsrv::DownloadError::ClientCreationFailed(e) => {
                DownloadError::ClientCreationFailed(e)
            }
            symsrv::DownloadError::OpenFailed(e) => DownloadError::OpenFailed(e),
            symsrv::DownloadError::Timeout => DownloadError::Timeout,
            symsrv::DownloadError::StatusError(status_code) => {
                DownloadError::StatusError(status_code.as_u16())
            }
            symsrv::DownloadError::CouldNotCreateDestinationDirectory => {
                DownloadError::CouldNotCreateDestinationDirectory
            }
            symsrv::DownloadError::UnexpectedContentEncoding(e) => {
                DownloadError::UnexpectedContentEncoding(e)
            }
            symsrv::DownloadError::ErrorDuringDownloading(e) => DownloadError::StreamRead(e),
            symsrv::DownloadError::ErrorWhileWritingDownloadedFile(e) => {
                DownloadError::DiskWrite(e)
            }
            symsrv::DownloadError::Redirect(e) => DownloadError::Redirect(e),
            symsrv::DownloadError::Other(e) => DownloadError::Other(e),
        };
        observer.on_download_failed(download_id, err);
    }

    fn on_download_canceled(&self, symsrv_download_id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .symsrv_download_id_mapping
            .remove(&symsrv_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_download_canceled(download_id);
    }

    fn on_new_cab_extraction(&self, _extraction_id: u64, _dest_path: &Path) {}
    fn on_cab_extraction_progress(
        &self,
        _extraction_id: u64,
        _bytes_so_far: u64,
        _total_bytes: u64,
    ) {
    }
    fn on_cab_extraction_completed(
        &self,
        _extraction_id: u64,
        _uncompressed_size_in_bytes: u64,
        _time_until_completed: std::time::Duration,
    ) {
    }
    fn on_cab_extraction_failed(&self, _extraction_id: u64, _reason: symsrv::CabExtractionError) {}
    fn on_cab_extraction_canceled(&self, _extraction_id: u64) {}

    fn on_file_created(&self, path: &Path, size_in_bytes: u64) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_file_created(path, size_in_bytes);
    }

    fn on_file_accessed(&self, path: &Path) {
        self.on_file_accessed(path);
    }

    fn on_file_missed(&self, path: &Path) {
        self.on_file_missed(path);
    }
}

impl DownloaderObserver for HelperDownloaderObserver {
    fn on_new_download_before_connect(&self, downloader_download_id: u64, url: &str) {
        let download_id = NEXT_DOWNLOAD_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        inner
            .downloader_download_id_mapping
            .insert(downloader_download_id, download_id);
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_new_download_before_connect(download_id, url);
    }

    fn on_download_started(&self, downloader_download_id: u64) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        let download_id = inner.downloader_download_id_mapping[&downloader_download_id];
        drop(inner);
        observer.on_download_started(download_id);
    }

    fn on_download_progress(
        &self,
        downloader_download_id: u64,
        bytes_so_far: u64,
        total_bytes: Option<u64>,
    ) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        let download_id = inner.downloader_download_id_mapping[&downloader_download_id];
        drop(inner);
        observer.on_download_progress(download_id, bytes_so_far, total_bytes);
    }

    fn on_download_completed(
        &self,
        downloader_download_id: u64,
        uncompressed_size_in_bytes: u64,
        time_until_headers: std::time::Duration,
        time_until_completed: std::time::Duration,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .downloader_download_id_mapping
            .remove(&downloader_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_download_completed(
            download_id,
            uncompressed_size_in_bytes,
            time_until_headers,
            time_until_completed,
        );
    }

    fn on_download_failed(&self, downloader_download_id: u64, reason: DownloadError) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .downloader_download_id_mapping
            .remove(&downloader_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_download_failed(download_id, reason);
    }

    fn on_download_canceled(&self, downloader_download_id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let download_id = inner
            .downloader_download_id_mapping
            .remove(&downloader_download_id)
            .unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_download_canceled(download_id);
    }

    fn on_file_created(&self, path: &Path, size_in_bytes: u64) {
        let inner = self.inner.lock().unwrap();
        let Some(observer) = inner.observer.clone() else {
            return;
        };
        drop(inner);
        observer.on_file_created(path, size_in_bytes);
    }

    fn on_file_accessed(&self, path: &Path) {
        self.on_file_accessed(path);
    }

    fn on_file_missed(&self, path: &Path) {
        self.on_file_missed(path);
    }
}

// Thoughts on logging
//
// The purpose of any logging in this file is to make it easier to diagnose missing symbols.
// However, it's super hard to know which pieces of information to log. Symbol lookup is
// basically a long sequence of "try lots of things until we find something". Many individual
// steps in this sequence are expected to fail in the normal case.
//
// When deciding whether something is worth logging, it's best to have a list of scenarios.
// So here's a list of scenarios. "I expected to see symbols, but I didn't see symbols."
//
// 1. No debug information for local rust binary: I have a release build but I forgot to
// specify debug = "true" in my Cargo.toml.
// 2. No macOS system library symbols on new macOS version due to broken dyld cache parsing:
// I am using a new macOS version and wholesym's parsing of the dyld shared cache hasn't
// been updated for the new format.
// 3. Running out of disk space for symbol files from server: I am getting symbols from a
// server (symsrv, breakpad, debuginfod), I have found the correct symbol file, but the
// download failed because my disk filled up.
// 4. Messed up environment variable syntax in Windows terminal: I wanted to get pdb symbols
// from a symbol server, but didn't set my _NT_SYMBOL_PATH environment variable correctly
// (or forgot to set it altogether) and I'm not getting Windows system library symbols or
// Firefox / Chrome symbols.
// 5. Symbols missing on server: I am profiling a build for which I expected symbols to be
// available on a symbol server but they weren't there. For example a Firefox try build, or
// a Windows driver.
// 6. Local files have changed after profiling, e.g. build ID no longer matches.
// 7. Invalid characters in library names, causing downloads to fail because the file paths
// where the downloaded files should be stored aren't valid.
//
// More generally, I want to know:
//  - Did it attempt to use the local files that I think it needs to use?
//  - Did it contact the symbol server I wanted it to contact?
//  - Did the download succeed? If not, was it the server's fault or my machine's fault (full disk)?
//
// I think it's ok if the logging here doesn't answer all those questions. Instead, the
// questions can be answered by information in the response JSON... or I guess by something
// that's stored on the SymbolMap.
