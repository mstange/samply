//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//! It maps raw code addresses to symbol strings, and, if available, file name + line number
//! information.
//! The API was designed for the Firefox profiler.
//!
//! The main entry point of this crate is the `SymbolManager` struct and its async `get_symbol_map` method.
//!
//! # Design constraints
//!
//! This crate operates under the following design constraints:
//!
//!   - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
//!     WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
//!     This setup allows us to download the profiler-get-symbols wasm bundle on demand, rather than shipping
//!     it with Firefox, which would increase the Firefox download size for a piece of functionality
//!     that the vast majority of Firefox users don't need.
//!   - Performance: We want to be able to obtain symbol data from a fresh build of a locally compiled
//!     Firefox instance as quickly as possible, without an expensive preprocessing step. The time between
//!     "finished compilation" and "returned symbol data" should be minimized. This means that symbol
//!     data needs to be obtained directly from the compilation artifacts rather than from, say, a
//!     dSYM bundle or a Breakpad .sym file.
//!   - Must scale to large inputs: This applies to both the size of the API request and the size of the
//!     object files that need to be parsed: The Firefox profiler will supply anywhere between tens of
//!     thousands and hundreds of thousands of different code addresses in a single symbolication request.
//!     Firefox build artifacts such as libxul.so can be multiple gigabytes big, and contain around 300000
//!     function symbols. We want to serve such requests within a few seconds or less.
//!   - "Best effort" basis: If only limited symbol information is available, for example from system
//!     libraries, we want to return whatever limited information we have.
//!
//! The WebAssembly requirement means that this crate cannot contain any direct file access.
//! Instead, all file access is mediated through a `FileAndPathHelper` trait which has to be implemented
//! by the caller. Furthermore, the API request does not carry any absolute file paths, so the resolution
//! to absolute file paths needs to be done by the caller as well.
//!
//! # Supported formats and data
//!
//! This crate supports obtaining symbol data from PE binaries (Windows), PDB files (Windows),
//! mach-o binaries (including fat binaries) (macOS & iOS), and ELF binaries (Linux, Android, etc.).
//! For mach-o files it also supports finding debug information in external objects, by following
//! OSO stabs entries.
//! It supports gathering both basic symbol information (function name strings) as well as information
//! based on debug data, i.e. inline callstacks where each frame has a function name, a file name,
//! and a line number.
//! For debug data we support both DWARF debug data (inside mach-o and ELF binaries) and PDB debug data.
//!
//! # Example
//!
//! ```rust
//! use samply_symbols::debugid::DebugId;
//! use samply_symbols::{
//!     CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
//!     FramesLookupResult, LibraryInfo, OptionallySendFuture, SymbolManager,
//! };
//!
//! async fn run_query() {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci"),
//!     };
//!
//!     let symbol_manager = SymbolManager::with_helper(&helper);
//!
//!     let library_info = LibraryInfo {
//!         debug_name: Some("firefox.pdb".to_string()),
//!         debug_id: DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").ok(),
//!         ..Default::default()
//!     };
//!     let symbol_map = match symbol_manager.load_symbol_map(&library_info).await {
//!         Ok(symbol_map) => symbol_map,
//!         Err(e) => {
//!             println!("Error while loading the symbol map: {:?}", e);
//!             return;
//!         }
//!     };
//!
//!     // Look up the symbol for an address.
//!     let lookup_result = symbol_map.lookup(0x1f98f);
//!
//!     match lookup_result {
//!         Some(address_info) => {
//!             // Print the symbol name for this address:
//!             println!("0x1f98f: {}", address_info.symbol.name);
//!
//!             // See if we have debug info (file name + line, and inlined frames):
//!             match address_info.frames {
//!                 FramesLookupResult::Available(frames) => {
//!                     println!("Debug info:");
//!                     for frame in frames {
//!                         println!(
//!                             " - {:?} ({:?}:{:?})",
//!                             frame.function, frame.file_path, frame.line_number
//!                         );
//!                     }
//!                 }
//!                 FramesLookupResult::External(ext_address) => {
//!                     // Debug info is located in a different file.
//!                     if let Some(frames) =
//!                         symbol_manager.lookup_external(&ext_address).await
//!                     {
//!                         println!("Debug info:");
//!                         for frame in frames {
//!                             println!(
//!                                 " - {:?} ({:?}:{:?})",
//!                                 frame.function, frame.file_path, frame.line_number
//!                             );
//!                         }
//!                     }
//!                 }
//!                 FramesLookupResult::Unavailable => {}
//!             }
//!         }
//!         None => {
//!             println!("No symbol was found for address 0x1f98f.")
//!         }
//!     }
//! }
//!
//! struct ExampleHelper {
//!     artifact_directory: std::path::PathBuf,
//! }
//!
//! impl<'h> FileAndPathHelper<'h> for ExampleHelper {
//!     type F = Vec<u8>;
//!     type OpenFileFuture = std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     >;
//!
//!     fn get_candidate_paths_for_debug_file(
//!         &self,
//!         library_info: &LibraryInfo,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         if let Some(debug_name) = library_info.debug_name.as_deref() {
//!             Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(
//!                 self.artifact_directory.join(debug_name),
//!             ))])
//!         } else {
//!             Ok(vec![])
//!         }
//!     }
//!
//!     fn get_candidate_paths_for_binary(
//!         &self,
//!         library_info: &LibraryInfo,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         if let Some(name) = library_info.name.as_deref() {
//!             Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(
//!                 self.artifact_directory.join(name),
//!             ))])
//!         } else {
//!             Ok(vec![])
//!         }
//!     }
//!
//!    fn get_dyld_shared_cache_paths(
//!        &self,
//!        _arch: Option<&str>,
//!    ) -> FileAndPathHelperResult<Vec<std::path::PathBuf>> {
//!        Ok(vec![])
//!    }
//!
//!     fn open_file(
//!         &'h self,
//!         location: &FileLocation,
//!     ) -> std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     > {
//!         async fn read_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
//!             Ok(std::fs::read(&path)?)
//!         }
//!
//!         let path = match location {
//!             FileLocation::Path(path) => path.clone(),
//!             FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
//!         };
//!         Box::pin(read_file_impl(path))
//!     }
//! }
//! ```

use std::path::Path;
use std::sync::Mutex;

use binary_image::BinaryImageInner;
pub use debugid;
pub use object;
pub use pdb_addr2line::pdb;

use object::read::FileKind;

mod binary_image;
mod breakpad;
mod cache;
mod chunked_read_buffer_manager;
mod compact_symbol_table;
mod debugid_util;
mod demangle;
mod demangle_ocaml;
mod dwarf;
mod elf;
mod error;
mod external_file;
mod macho;
mod path_mapper;
mod shared;
mod symbol_map;
mod symbol_map_object;
mod windows;

pub use crate::binary_image::BinaryImage;
pub use crate::cache::{FileByteSource, FileContentsWithChunkedCaching};
pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::debugid_util::{debug_id_for_object, DebugIdExt};
pub use crate::error::Error;
pub use crate::external_file::{load_external_file, ExternalFileSymbolMap};
pub use crate::macho::FatArchiveMember;
pub use crate::shared::{
    relative_address_base, AddressDebugInfo, AddressInfo, CandidatePathInfo, CodeId, ElfBuildId,
    ExternalFileAddressInFileRef, ExternalFileAddressRef, ExternalFileRef, FileAndPathHelper,
    FileAndPathHelperError, FileAndPathHelperResult, FileContents, FileContentsWrapper,
    FileLocation, FilePath, FramesLookupResult, InlineStackFrame, LibraryInfo,
    MultiArchDisambiguator, OptionallySendFuture, PeCodeId, SymbolInfo, BasePath,
};
pub use crate::symbol_map::SymbolMap;

pub struct SymbolManager<'h, H: FileAndPathHelper<'h>> {
    helper: &'h H,
    cached_external_file: Mutex<Option<ExternalFileSymbolMap>>,
}

impl<'h, H, F> SymbolManager<'h, H>
where
    H: FileAndPathHelper<'h, F = F>,
    F: FileContents + 'static,
{
    // Create a new `SymbolManager`.
    pub fn with_helper(helper: &'h H) -> Self {
        Self {
            helper,
            cached_external_file: Mutex::new(None),
        }
    }

    /// Exposes the helper.
    pub fn helper(&self) -> &'h H {
        self.helper
    }

    async fn load_symbol_map_at_candidate_path(
        &self,
        candidate_path: &CandidatePathInfo,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap, Error> {
        match candidate_path {
            CandidatePathInfo::SingleFile(file_location) => {
                self.load_symbol_map_from_location(file_location, multi_arch_disambiguator)
                    .await
            }
            CandidatePathInfo::InDyldCache {
                dyld_cache_path,
                dylib_path,
            } => {
                macho::load_symbol_map_for_dyld_cache(dyld_cache_path, dylib_path, self.helper)
                    .await
            }
        }
    }

    /// Obtain a symbol map for the library, given the (partial) `LibraryInfo`.
    /// At least the debug_id has to be given.
    pub async fn load_symbol_map(&self, library_info: &LibraryInfo) -> Result<SymbolMap, Error> {
        let debug_id = match library_info.debug_id {
            Some(debug_id) => debug_id,
            None => return Err(Error::NotEnoughInformationToIdentifySymbolMap),
        };

        let candidate_paths = self
            .helper
            .get_candidate_paths_for_debug_file(library_info)
            .map_err(|e| {
                Error::HelperErrorDuringGetCandidatePathsForDebugFile(library_info.clone(), e)
            })?;

        let mut last_err = None;
        for candidate_info in candidate_paths {
            let symbol_map = self
                .load_symbol_map_at_candidate_path(
                    &candidate_info,
                    Some(MultiArchDisambiguator::DebugId(debug_id)),
                )
                .await;

            match symbol_map {
                Ok(symbol_map) if symbol_map.debug_id() == debug_id => return Ok(symbol_map),
                Ok(symbol_map) => {
                    last_err = Some(Error::UnmatchedDebugId(symbol_map.debug_id(), debug_id));
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::NoCandidatePathForDebugFile(library_info.clone())))
    }

    /// Load and return an external file which may contain additional debug info.
    ///
    /// This is used on macOS: When linking multiple `.o` files together into a library or
    /// an executable, the linker does not copy the dwarf sections into the linked output.
    /// Instead, it stores the paths to those original `.o` files, using OSO stabs entries.
    ///
    /// A `SymbolMap` for such a linked file will not find debug info, and will return
    /// `FramesLookupResult::External` from the lookups. Then the address needs to be
    /// looked up in the external file.
    ///
    /// Also see `SymbolManager::lookup_external`.
    pub async fn load_external_file(
        &self,
        external_file_ref: &ExternalFileRef,
    ) -> Result<ExternalFileSymbolMap, Error> {
        external_file::load_external_file(self.helper, external_file_ref).await
    }

    /// Resolve a debug info lookup for which `SymbolMap::lookup` returned a
    /// `FramesLookupResult::External`.
    ///
    /// This method is asynchronous because it may load a new external file.
    ///
    /// This keeps the most recent external file cached, so that repeated lookups
    /// for the same external file are fast.
    pub async fn lookup_external(
        &self,
        address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        {
            let cached_external_file = self.cached_external_file.lock().ok()?;
            match &*cached_external_file {
                Some(external_file) if external_file.is_same_file(&address.file_ref) => {
                    return external_file.lookup(&address.address_in_file);
                }
                _ => {}
            }
        }

        let external_file = self.load_external_file(&address.file_ref).await.ok()?;
        let lookup_result = external_file.lookup(&address.address_in_file);

        if let Ok(mut guard) = self.cached_external_file.lock() {
            *guard = Some(external_file);
        }
        lookup_result
    }

    async fn load_binary_from_candidate_path(
        &self,
        candidate_path: &CandidatePathInfo,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<BinaryImage<F>, Error> {
        match candidate_path {
            CandidatePathInfo::SingleFile(file_location) => {
                self.load_binary_at_location(file_location, multi_arch_disambiguator)
                    .await
            }
            CandidatePathInfo::InDyldCache {
                dyld_cache_path,
                dylib_path,
            } => {
                self.load_binary_from_dyld_cache(dyld_cache_path, dylib_path)
                    .await
            }
        }
    }

    async fn load_binary_from_dyld_cache(
        &self,
        dyld_cache_path: &Path,
        dylib_path: &str,
    ) -> Result<BinaryImage<F>, Error> {
        let file_data =
            macho::load_file_data_for_dyld_cache(dyld_cache_path, dylib_path, self.helper).await?;
        let inner = BinaryImageInner::MemberOfDyldSharedCache(file_data);
        let image = BinaryImage::new(inner, Some(Path::new(&dylib_path)), FileKind::DyldCache)?;
        Ok(image)
    }

    /// Returns the binary for the given (partial) [`LibraryInfo`].
    ///
    /// This consults the helper to get candidate paths to the binary.
    pub async fn load_binary(&self, info: &LibraryInfo) -> Result<BinaryImage<F>, Error> {
        // Require at least either the code ID or a (debug_name, debug_id) pair.
        if info.code_id.is_none() && (info.debug_name.is_none() || info.debug_id.is_none()) {
            return Err(Error::NotEnoughInformationToIdentifyBinary);
        }

        let candidate_paths_for_binary = self
            .helper
            .get_candidate_paths_for_binary(info)
            .map_err(|e| Error::HelperErrorDuringGetCandidatePathsForBinary(e))?;

        let disambiguator = match (&info.debug_id, &info.arch) {
            (Some(debug_id), _) => Some(MultiArchDisambiguator::DebugId(*debug_id)),
            (None, Some(arch)) => Some(MultiArchDisambiguator::Arch(arch.clone())),
            (None, None) => None,
        };

        let mut last_err = None;
        for candidate_info in candidate_paths_for_binary {
            let image = self
                .load_binary_from_candidate_path(&candidate_info, disambiguator.clone())
                .await;

            match image {
                Ok(image) => {
                    let e = if let Some(expected_debug_id) = info.debug_id {
                        if image.debug_id() == Some(expected_debug_id) {
                            return Ok(image);
                        }
                        Error::UnmatchedDebugIdOptional(expected_debug_id, image.debug_id())
                    } else if let Some(expected_code_id) = info.code_id.as_ref() {
                        if image.code_id().as_ref() == Some(expected_code_id) {
                            return Ok(image);
                        }
                        Error::UnmatchedCodeId(expected_code_id.clone(), image.code_id())
                    } else {
                        panic!(
                            "We checked earlier that we have at least one of debug_id / code_id."
                        )
                    };
                    last_err = Some(e);
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            Error::NoCandidatePathForBinary(info.debug_name.clone(), info.debug_id)
        }))
    }

    /// Returns the file data of the binary at the given path. This also consults the
    /// dyld shared cache if the file does not exist.
    pub async fn load_binary_at_path(
        &self,
        path: &Path,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<BinaryImage<F>, Error> {
        let might_be_in_dyld_shared_cache =
            path.starts_with("/usr/") || path.starts_with("/System/");

        let mut err = match self
            .load_binary_at_location(
                &FileLocation::Path(path.into()),
                multi_arch_disambiguator.clone(),
            )
            .await
        {
            Ok(binary) => return Ok(binary),
            Err(e @ Error::HelperErrorDuringOpenFile(_, _)) if might_be_in_dyld_shared_cache => e,
            Err(e) => return Err(e),
        };

        // The file at the given path could not be opened, so it probably doesn't exist.
        // Check the dyld cache.

        let arch = match &multi_arch_disambiguator {
            Some(MultiArchDisambiguator::Arch(arch)) => Some(arch.as_str()),
            _ => None,
        };
        let dyld_shared_cache_paths = self
            .helper
            .get_dyld_shared_cache_paths(arch)
            .map_err(Error::HelperErrorDuringGetDyldSharedCachePaths)?;

        let dylib_path = path.to_string_lossy();
        for dyld_cache_path in dyld_shared_cache_paths {
            match self
                .load_binary_from_dyld_cache(&dyld_cache_path, &dylib_path)
                .await
            {
                Ok(binary) => return Ok(binary),
                Err(e) => err = e,
            }
        }
        Err(err)
    }

    pub async fn load_symbol_map_from_path(
        &self,
        path: &Path,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap, Error> {
        let might_be_in_dyld_shared_cache =
            path.starts_with("/usr/") || path.starts_with("/System/");

        let mut err = match self
            .load_symbol_map_from_location(
                &FileLocation::Path(path.into()),
                multi_arch_disambiguator.clone(),
            )
            .await
        {
            Ok(symbol_map) => return Ok(symbol_map),
            Err(e @ Error::HelperErrorDuringOpenFile(_, _)) if might_be_in_dyld_shared_cache => e,
            Err(e) => return Err(e),
        };

        // The file at the given path could not be opened, so it probably doesn't exist.
        // Check the dyld cache.

        let arch = match &multi_arch_disambiguator {
            Some(MultiArchDisambiguator::Arch(arch)) => Some(arch.as_str()),
            _ => None,
        };
        let dyld_shared_cache_paths = self
            .helper
            .get_dyld_shared_cache_paths(arch)
            .map_err(Error::HelperErrorDuringGetDyldSharedCachePaths)?;

        let dylib_path = path.to_string_lossy();
        for dyld_cache_path in dyld_shared_cache_paths {
            match macho::load_symbol_map_for_dyld_cache(&dyld_cache_path, &dylib_path, self.helper)
                .await
            {
                Ok(symbol_map) => return Ok(symbol_map),
                Err(e) => err = e,
            }
        }
        Err(err)
    }

    async fn load_symbol_map_from_location(
        &self,
        file_location: &FileLocation,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap, Error> {
        let file_contents =
            self.helper.open_file(file_location).await.map_err(|e| {
                Error::HelperErrorDuringOpenFile(file_location.to_string_lossy(), e)
            })?;
        let base_path = file_location.to_base_path();

        let file_contents = FileContentsWrapper::new(file_contents);

        if let Ok(file_kind) = FileKind::parse(&file_contents) {
            match file_kind {
                FileKind::Elf32 | FileKind::Elf64 => {
                    elf::load_symbol_map_for_elf(file_contents, file_kind, &base_path, self.helper)
                        .await
                }
                FileKind::MachOFat32 | FileKind::MachOFat64 => {
                    let range = macho::get_fat_archive_member_range(
                        &file_contents,
                        file_kind,
                        multi_arch_disambiguator,
                    )?;
                    macho::get_symbol_map_for_fat_archive_member(&base_path, file_contents, range)
                }
                FileKind::MachO32 | FileKind::MachO64 => {
                    macho::get_symbol_map_for_macho(&base_path, file_contents)
                }
                FileKind::Pe32 | FileKind::Pe64 => {
                    match windows::load_symbol_map_for_pdb_corresponding_to_binary(
                        file_kind,
                        &file_contents,
                        file_location,
                        self.helper,
                    )
                    .await
                    {
                        Ok(symbol_map) => Ok(symbol_map),
                        Err(_) => {
                            windows::get_symbol_map_for_pe(file_contents, file_kind, &base_path)
                        }
                    }
                }
                _ => Err(Error::InvalidInputError(
                    "Input was Archive, Coff or Wasm format, which are unsupported for now",
                )),
            }
        } else if windows::is_pdb_file(&file_contents) {
            windows::get_symbol_map_for_pdb(file_contents, &base_path)
        } else if breakpad::is_breakpad_file(&file_contents) {
            breakpad::get_symbol_map_for_breakpad_sym(file_contents)
        } else {
            Err(Error::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
        }
    }

    async fn load_binary_at_location(
        &self,
        file_location: &FileLocation,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<BinaryImage<F>, Error> {
        let file_contents =
            self.helper.open_file(file_location).await.map_err(|e| {
                Error::HelperErrorDuringOpenFile(file_location.to_string_lossy(), e)
            })?;

        let file_contents = FileContentsWrapper::new(file_contents);

        let file_kind = FileKind::parse(&file_contents)
            .map_err(|_| Error::InvalidInputError("Unrecognized file"))?;
        let inner = match file_kind {
            FileKind::Elf32
            | FileKind::Elf64
            | FileKind::MachO32
            | FileKind::MachO64
            | FileKind::Pe32
            | FileKind::Pe64 => BinaryImageInner::Normal(file_contents),
            FileKind::MachOFat32 | FileKind::MachOFat64 => {
                let (offset, size) = macho::get_fat_archive_member_range(
                    &file_contents,
                    file_kind,
                    multi_arch_disambiguator,
                )?;
                let data = macho::MachOFatArchiveMemberData::new(file_contents, offset, size);
                BinaryImageInner::MemberOfFatArchive(data)
            }
            _ => {
                return Err(Error::InvalidInputError(
                    "Input was Archive, Coff or Wasm format, which are unsupported for now",
                ))
            }
        };
        let path = match file_location {
            FileLocation::Path(p) => Some(p.as_path()),
            FileLocation::Custom(_) => None,
        };
        BinaryImage::new(inner, path, file_kind)
    }
}
