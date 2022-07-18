pub use samply_symbols::debugid;
use samply_symbols::debugid::DebugId;
use samply_symbols::{
    self, CandidatePathInfo, CompactSymbolTable, FileAndPathHelper, FileAndPathHelperResult,
    FileLocation, GetSymbolsError, OptionallySendFuture,
};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;

#[cfg(feature = "chunked_caching")]
use samply_symbols::{FileByteSource, FileContents};

pub async fn get_table(
    debug_name: &str,
    debug_id: Option<DebugId>,
    symbol_directory: PathBuf,
) -> anyhow::Result<CompactSymbolTable> {
    let helper = Helper { symbol_directory };
    let table = get_symbols_retry_id(debug_name, debug_id, &helper).await?;
    Ok(table)
}

async fn get_symbols_retry_id(
    debug_name: &str,
    debug_id: Option<DebugId>,
    helper: &Helper,
) -> anyhow::Result<CompactSymbolTable> {
    let debug_id = match debug_id {
        Some(debug_id) => debug_id,
        None => {
            // No debug ID was specified. get_compact_symbol_table always wants one, so we call it twice:
            // First, with a bogus debug ID (DebugId::nil()), and then again with the debug ID that
            // it expected.
            let result =
                samply_symbols::get_compact_symbol_table(debug_name, DebugId::nil(), helper).await;
            match result {
                Ok(table) => return Ok(table),
                Err(err) => match err {
                    GetSymbolsError::UnmatchedDebugId(expected, supplied)
                        if supplied == DebugId::nil() =>
                    {
                        eprintln!("Using debug ID: {}", expected.breakpad());
                        expected
                    }
                    err => return Err(err.into()),
                },
            }
        }
    };
    Ok(samply_symbols::get_compact_symbol_table(debug_name, debug_id, helper).await?)
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
        writeln!(w, "{:x} {}", address, symbol_string)?;
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

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContentsType;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        _breakpad_id: &DebugId,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let mut paths = vec![];

        // Also consider .so.dbg files in the symbol directory.
        if debug_name.ends_with(".so") {
            let debug_debug_name = format!("{}.dbg", debug_name);
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                self.symbol_directory.join(debug_debug_name),
            )));
        }

        // And dSYM packages.
        if !debug_name.ends_with(".pdb") {
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                self.symbol_directory
                    .join(&format!("{}.dSYM", debug_name))
                    .join("Contents")
                    .join("Resources")
                    .join("DWARF")
                    .join(debug_name),
            )));
        }

        // Finally, the file itself.
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            self.symbol_directory.join(debug_name),
        )));

        // For macOS system libraries, also consult the dyld shared cache.
        if self.symbol_directory.starts_with("/usr/")
            || self.symbol_directory.starts_with("/System/")
        {
            if let Some(dylib_path) = self.symbol_directory.join(debug_name).to_str() {
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_arm64e")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64h")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64")
                        .to_path_buf(),
                    dylib_path: dylib_path.to_string(),
                });
            }
        }

        Ok(paths)
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        async fn open_file_impl(path: PathBuf) -> FileAndPathHelperResult<FileContentsType> {
            eprintln!("Opening file {:?}", &path);
            let file = File::open(&path)?;
            let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
            Ok(mmap_to_file_contents(mmap))
        }

        let path = match location {
            FileLocation::Path(path) => path.clone(),
            FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
        };
        Box::pin(open_file_impl(path))
    }
}
