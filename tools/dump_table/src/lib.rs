pub use samply_symbols::debugid;
use samply_symbols::debugid::DebugId;
use samply_symbols::{
    self, CandidatePathInfo, CompactSymbolTable, Error, FileAndPathHelper, FileAndPathHelperResult,
    FileLocation, LibraryInfo, MultiArchDisambiguator, SymbolManager,
};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[cfg(feature = "chunked_caching")]
use samply_symbols::{FileByteSource, FileContents};

use samply_symbols::BinaryImage;

async fn get_library_info_with_dyld_cache_fallback(
    symbol_manager: &SymbolManager<Helper>,
    path: &Path,
    debug_id: Option<DebugId>,
) -> Result<BinaryImage<FileContentsType>, Error> {
    let might_be_in_dyld_shared_cache = path.starts_with("/usr/") || path.starts_with("/System/");

    let disambiguator = debug_id.map(MultiArchDisambiguator::DebugId);
    let location = FileLocationType(path.to_owned());
    let name = path
        .file_name()
        .and_then(|name| Some(name.to_str()?.to_owned()));
    let path_str = path.to_str().map(ToOwned::to_owned);

    match symbol_manager
        .load_binary_at_location(location, name, path_str, disambiguator.clone())
        .await
    {
        Ok(binary) => Ok(binary),
        Err(Error::HelperErrorDuringOpenFile(_, _)) if might_be_in_dyld_shared_cache => {
            // The file at the given path could not be opened, so it probably doesn't exist.
            // Check the dyld cache.
            symbol_manager
                .load_binary_for_dyld_cache_image(&path.to_string_lossy(), disambiguator)
                .await
        }
        Err(e) => Err(e),
    }
}

pub async fn get_table_for_binary(
    binary_path: &Path,
    debug_id: Option<DebugId>,
) -> Result<CompactSymbolTable, Error> {
    let helper = Helper {
        symbol_directory: binary_path.parent().unwrap().to_path_buf(),
    };
    let symbol_manager = SymbolManager::with_helper(helper);
    let binary =
        get_library_info_with_dyld_cache_fallback(&symbol_manager, binary_path, debug_id).await?;
    let info = binary.library_info();
    drop(binary);
    let symbol_map = symbol_manager.load_symbol_map(&info).await?;
    Ok(CompactSymbolTable::from_symbol_map(&symbol_map))
}

pub async fn get_table_for_debug_name_and_id(
    debug_name: &str,
    debug_id: Option<DebugId>,
    symbol_directory: PathBuf,
) -> Result<CompactSymbolTable, Error> {
    let helper = Helper { symbol_directory };
    let symbol_manager = SymbolManager::with_helper(helper);
    let info = LibraryInfo {
        debug_name: Some(debug_name.to_string()),
        debug_id,
        ..Default::default()
    };
    let symbol_map = symbol_manager.load_symbol_map(&info).await?;
    Ok(CompactSymbolTable::from_symbol_map(&symbol_map))
}

pub fn dump_table(w: &mut impl Write, table: CompactSymbolTable, full: bool) -> anyhow::Result<()> {
    let mut w = BufWriter::new(w);
    writeln!(w, "Found {} symbols.", table.addr.len())?;
    for (i, address) in table.addr.iter().enumerate() {
        if i >= 15 && !full {
            writeln!(
                w,
                "and {} more symbols. Pass --full to print the full list.",
                table.addr.len() - i
            )?;
            break;
        }

        let start_pos = table.index[i];
        let end_pos = table.index[i + 1];
        let symbol_bytes = &table.buffer[start_pos as usize..end_pos as usize];
        let symbol_string = std::str::from_utf8(symbol_bytes)?;
        writeln!(w, "{address:x} {symbol_string}")?;
    }
    Ok(())
}

struct Helper {
    symbol_directory: PathBuf,
}

#[cfg(feature = "chunked_caching")]
struct MmapFileContents(memmap2::Mmap);

#[cfg(feature = "chunked_caching")]
impl FileByteSource for MmapFileContents {
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()> {
        self.0.read_bytes_into(buffer, offset, size)
    }
}

#[cfg(feature = "chunked_caching")]
type FileContentsType = samply_symbols::FileContentsWithChunkedCaching<MmapFileContents>;

#[cfg(feature = "chunked_caching")]
fn mmap_to_file_contents(mmap: memmap2::Mmap) -> FileContentsType {
    samply_symbols::FileContentsWithChunkedCaching::new(mmap.len() as u64, MmapFileContents(mmap))
}

#[cfg(not(feature = "chunked_caching"))]
type FileContentsType = memmap2::Mmap;

#[cfg(not(feature = "chunked_caching"))]
fn mmap_to_file_contents(m: memmap2::Mmap) -> FileContentsType {
    m
}

impl FileAndPathHelper for Helper {
    type F = FileContentsType;
    type FL = FileLocationType;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<FileLocationType>>> {
        let debug_name = match library_info.debug_name.as_deref() {
            Some(debug_name) => debug_name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Check .so.dbg files in the symbol directory.
        if debug_name.ends_with(".so") {
            let debug_debug_name = format!("{debug_name}.dbg");
            paths.push(CandidatePathInfo::SingleFile(FileLocationType(
                self.symbol_directory.join(debug_debug_name),
            )));
        }

        // And dSYM packages.
        if !debug_name.ends_with(".pdb") {
            paths.push(CandidatePathInfo::SingleFile(FileLocationType(
                self.symbol_directory
                    .join(format!("{debug_name}.dSYM"))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            )));
        }

        // Finally, the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocationType(
            self.symbol_directory.join(debug_name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(debug_name).to_str() {
                paths.extend(
                    self.get_dyld_shared_cache_paths(None)
                        .unwrap()
                        .into_iter()
                        .map(|dyld_cache_path| CandidatePathInfo::InDyldCache {
                            dyld_cache_path,
                            dylib_path: dylib_path.to_string(),
                        }),
                );
            }
        }

        Ok(paths)
    }

    fn get_dyld_shared_cache_paths(
        &self,
        _arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<FileLocationType>> {
        Ok(vec![
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_arm64e"),
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64h"),
            FileLocationType::new("/System/Library/dyld/dyld_shared_cache_x86_64"),
        ])
    }

    async fn load_file(&self, location: FileLocationType) -> FileAndPathHelperResult<Self::F> {
        let mut path = location.0;

        if !path.starts_with(&self.symbol_directory) {
            // See if this file exists in self.symbol_directory.
            // For example, when looking up object files referenced by mach-O binaries,
            // we want to take the object files from the symbol directory if they exist,
            // rather than from the original path.
            if let Some(filename) = path.file_name() {
                let redirected_path = self.symbol_directory.join(filename);
                if std::fs::metadata(&redirected_path).is_ok() {
                    // redirected_path exists!
                    eprintln!("Redirecting {:?} to {:?}", &path, &redirected_path);
                    path = redirected_path;
                }
            }
        }

        eprintln!("Reading file {:?}", &path);
        let file = File::open(&path)?;
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
        Ok(mmap_to_file_contents(mmap))
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<FileLocationType>>> {
        let name = match library_info.name.as_deref() {
            Some(name) => name,
            None => return Ok(Vec::new()),
        };

        let mut paths = vec![];

        // Start with the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocationType(
            self.symbol_directory.join(name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(name).to_str() {
                paths.extend(
                    self.get_dyld_shared_cache_paths(None)
                        .unwrap()
                        .into_iter()
                        .map(|dyld_cache_path| CandidatePathInfo::InDyldCache {
                            dyld_cache_path,
                            dylib_path: dylib_path.to_string(),
                        }),
                );
            }
        }

        Ok(paths)
    }
}

#[derive(Clone)]
struct FileLocationType(PathBuf);

impl FileLocationType {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

impl std::fmt::Display for FileLocationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.to_string_lossy().fmt(f)
    }
}

impl FileLocation for FileLocationType {
    fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self> {
        let mut filename = self.0.file_name().unwrap().to_owned();
        filename.push(suffix);
        Some(Self(self.0.with_file_name(filename)))
    }

    fn location_for_external_object_file(&self, object_file: &str) -> Option<Self> {
        Some(Self(object_file.into()))
    }

    fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self> {
        Some(Self(pdb_path_in_binary.into()))
    }

    fn location_for_source_file(&self, source_file_path: &str) -> Option<Self> {
        Some(Self(source_file_path.into()))
    }

    fn location_for_breakpad_symindex(&self) -> Option<Self> {
        Some(Self(self.0.with_extension("symindex")))
    }

    fn location_for_dwo(&self, _comp_dir: &str, _path: &str) -> Option<Self> {
        None // TODO
    }

    fn location_for_dwp(&self) -> Option<Self> {
        let mut s = self.0.as_os_str().to_os_string();
        s.push(".dwp");
        Some(Self(s.into()))
    }
}
