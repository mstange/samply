use debugid::DebugId;
use samply_api::samply_symbols::{self, BasePath, ElfBuildId, LibraryInfo, PeCodeId};
use samply_symbols::{
    CandidatePathInfo, CodeId, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    OptionallySendFuture,
};
use symsrv::{memmap2, FileContents, SymbolCache};
use uuid::Uuid;

use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

use crate::{config::SymbolManagerConfig, debuginfod::DebuginfodSymbolCache};

/// A simple helper which only exists to let samply_symbols::SymbolManager open
/// local files for the binary_at_path functions.
pub struct FileReadOnlyHelper;

impl FileReadOnlyHelper {
    async fn open_file_impl(
        &self,
        location: FileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            FileLocation::Path(path) => {
                let file = File::open(path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            FileLocation::Custom(_) => {
                panic!("FileLocation::Custom should not be hit in FileReadOnlyHelper");
            }
        }
    }
}

impl<'h> FileAndPathHelper<'h> for FileReadOnlyHelper {
    type F = FileContents;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        _library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        panic!("Should not be called");
    }

    fn get_candidate_paths_for_binary(
        &self,
        _library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        panic!("Should not be called");
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.open_file_impl(location.clone()))
    }

    fn get_dyld_shared_cache_paths(
        &self,
        arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<PathBuf>> {
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

    async fn open_file_impl(
        &self,
        location: FileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            FileLocation::Path(path) => {
                if self.config.verbose {
                    eprintln!("Opening file {:?}", path.to_string_lossy());
                }
                let path = self.config.redirect_paths.get(&path).unwrap_or(&path);
                let file = File::open(path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            FileLocation::Custom(custom) => {
                if let Some(path) = custom.strip_prefix("winsymbolserver:") {
                    if self.config.verbose {
                        eprintln!("Trying to get file {:?} from symbol cache", path);
                    }
                    Ok(self
                        .win_symbol_cache
                        .as_ref()
                        .unwrap()
                        .get_file(Path::new(path))
                        .await?)
                } else if let Some(path) = custom.strip_prefix("bpsymbolserver:") {
                    if self.config.verbose {
                        eprintln!("Trying to get file {:?} from breakpad symbol server", path);
                    }
                    self.get_bp_sym_file(path).await
                } else if let Some(buildid) = custom.strip_prefix("debuginfod-debuginfo:") {
                    self.debuginfod_symbol_cache
                        .as_ref()
                        .unwrap()
                        .get_file(buildid, "debuginfo")
                        .await
                        .ok_or_else(|| "Debuginfod could not find debuginfo".into())
                } else if let Some(buildid) = custom.strip_prefix("debuginfod-executable:") {
                    self.debuginfod_symbol_cache
                        .as_ref()
                        .unwrap()
                        .get_file(buildid, "executable")
                        .await
                        .ok_or_else(|| "Debuginfod could not find executable".into())
                } else {
                    panic!("Unexpected custom path: {}", custom);
                }
            }
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
        let url = format!("{}/{}", server_base_url, rel_path);
        if self.config.verbose {
            eprintln!("Downloading {}...", url);
        }
        let sym_file_response = reqwest::get(&url).await?.error_for_status()?;
        let mut stream = sym_file_response.bytes_stream();
        let dest_path = cache_dir.join(rel_path);
        if let Some(dir) = dest_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        if self.config.verbose {
            eprintln!("Saving bytes to {:?}.", dest_path);
        }
        let file = tokio::fs::File::create(&dest_path).await?;
        let mut writer = tokio::io::BufWriter::new(file);
        use futures_util::StreamExt;
        while let Some(item) = stream.next().await {
            tokio::io::copy(&mut item?.as_ref(), &mut writer).await?;
        }
        drop(writer);
        if self.config.verbose {
            eprintln!("Opening file {:?}", dest_path.to_string_lossy());
        }
        let file = File::open(&dest_path)?;
        Ok(FileContents::Mmap(unsafe {
            memmap2::MmapOptions::new().map(&file)?
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

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContents;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
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
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dsym_path.clone(),
                    )));
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dsym_path
                            .join("Contents")
                            .join("Resources")
                            .join("DWARF")
                            .join(debug_name),
                    )));
                }
            }

            // Also consider .so.dbg files in the same directory.
            if debug_path.ends_with(".so") {
                let so_dbg_path = format!("{}.dbg", debug_path);
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    PathBuf::from(so_dbg_path),
                )));
            }

            if debug_path.ends_with(".pdb") {
                // Get symbols from the pdb file.
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    debug_path.into(),
                )));
            }
        }

        if !got_dsym {
            if let Some(debug_id) = info.debug_id {
                // Try a little harder to find a dSYM, just from the UUID. We can do this
                // even if we don't have an entry for this library in the libinfo map.
                if let Ok(dsym_path) =
                    crate::moria_mac::locate_dsym_using_spotlight(debug_id.uuid())
                {
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dsym_path.clone(),
                    )));
                    if let Some(dsym_file_name) = dsym_path.file_name().and_then(|s| s.to_str()) {
                        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                            dsym_path
                                .join("Contents")
                                .join("Resources")
                                .join("DWARF")
                                .join(dsym_file_name.trim_end_matches(".dSYM")),
                        )));
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
                let path = format!("/usr/lib/debug/.build-id/{}/{}.debug", two_chars, rest);
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    PathBuf::from(path),
                )));
            }
        }

        if let (Some(debug_name), Some(debug_id)) = (&info.debug_name, info.debug_id) {
            // Search breakpad symbol directories.
            for dir in &self.config.breakpad_directories_readonly {
                let bp_path = dir
                    .join(debug_name)
                    .join(debug_id.breakpad().to_string())
                    .join(format!("{}.sym", debug_name.trim_end_matches(".pdb")));
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(bp_path)));
            }

            for (_url, dir) in &self.config.breakpad_servers {
                let bp_path = dir
                    .join(debug_name)
                    .join(debug_id.breakpad().to_string())
                    .join(format!("{}.sym", debug_name.trim_end_matches(".pdb")));
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(bp_path)));
            }

            if debug_name.ends_with(".pdb") && self.win_symbol_cache.is_some() {
                // We might find this pdb file with the help of a symbol server.
                // Construct a custom string to identify this pdb.
                let custom = format!(
                    "winsymbolserver:{}/{}/{}",
                    debug_name,
                    debug_id.breakpad(),
                    debug_name
                );
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(custom)));
            }

            if !self.config.breakpad_servers.is_empty() {
                // We might find a .sym file on a symbol server.
                // Construct a custom string to identify this file.
                let custom = format!(
                    "bpsymbolserver:{}/{}/{}.sym",
                    debug_name,
                    debug_id.breakpad(),
                    debug_name.trim_end_matches(".pdb")
                );
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(custom)));
            }
        }

        if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
            (self.debuginfod_symbol_cache.as_ref(), &info.code_id)
        {
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(
                format!("debuginfod-debuginfo:{build_id}"),
            )));
        }

        if let Some(path) = &info.path {
            // Fall back to getting symbols from the binary itself.
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                path.into(),
            )));

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
        debug_link_name: &str,
    ) -> FileAndPathHelperResult<Vec<PathBuf>> {
        // https://www-zeuthen.desy.de/unix/unixguide/infohtml/gdb/Separate-Debug-Files.html
        Ok(vec![
            PathBuf::from(format!("/usr/bin/{}.debug", &debug_link_name)),
            PathBuf::from(format!("/usr/bin/.debug/{}.debug", &debug_link_name)),
            PathBuf::from(format!("/usr/lib/debug/usr/bin/{}.debug", &debug_link_name)),
        ])
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let mut info = library_info.clone();
        self.fill_in_library_info_details(&mut info);

        let mut paths = vec![];

        // Begin with the binary itself.
        if let Some(path) = &info.path {
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                path.into(),
            )));
        }

        if let (Some(_symbol_cache), Some(name), Some(CodeId::PeCodeId(code_id))) =
            (&self.win_symbol_cache, &info.name, &info.code_id)
        {
            // We might find this exe / dll file with the help of a symbol server.
            // Construct a custom string to identify this file.
            let custom = format!("winsymbolserver:{}/{}/{}", name, code_id, name);
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(custom)));
        }

        if let (Some(_debuginfod_symbol_cache), Some(CodeId::ElfBuildId(build_id))) =
            (self.debuginfod_symbol_cache.as_ref(), &info.code_id)
        {
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(
                format!("debuginfod-executable:{build_id}"),
            )));
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
    ) -> FileAndPathHelperResult<Vec<PathBuf>> {
        Ok(get_dyld_shared_cache_paths(arch))
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.open_file_impl(location.clone()))
    }

    fn get_candidate_paths_for_supplementary_debug_file(
        &self,
        original_file_path: &BasePath,
        sup_file_path: &str,
        sup_file_build_id: &ElfBuildId,
    ) -> FileAndPathHelperResult<Vec<FileLocation>> {
        let mut paths = Vec::new();

        match original_file_path {
            BasePath::CanReferToLocalFiles(parent_dir) => {
                // TODO: Test if this actually works
                if sup_file_path.starts_with('/') {
                    paths.push(FileLocation::Path(PathBuf::from(sup_file_path)));
                } else {
                    let sup_file_path = Path::new(sup_file_path);
                    paths.push(FileLocation::Path(parent_dir.join(sup_file_path)));
                }
            }
            BasePath::NoLocalSourceFileAccess => {}
        }

        let build_id = sup_file_build_id.to_string();
        if build_id.len() > 2 {
            let (two_chars, rest) = build_id.split_at(2);
            let path = format!("/usr/lib/debug/.build-id/{}/{}.debug", two_chars, rest);
            paths.push(FileLocation::Path(PathBuf::from(path)));

            paths.push(FileLocation::Custom(format!(
                "debuginfod-debuginfo:{build_id}"
            )));
        }

        Ok(paths)
    }
}

fn get_dyld_shared_cache_paths(arch: Option<&str>) -> Vec<PathBuf> {
    if let Some(arch) = arch {
        vec![
            format!(
                "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_{arch}"
            )
            .into(),
            format!("/System/Library/dyld/dyld_shared_cache_{arch}").into(),
        ]
    } else {
        vec![
            "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e"
                .into(),
            "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_x86_64"
                .into(),
            "/System/Library/dyld/dyld_shared_cache_arm64e".into(),
            "/System/Library/dyld/dyld_shared_cache_x86_64h".into(),
            "/System/Library/dyld/dyld_shared_cache_x86_64".into(),
        ]
    }
}
