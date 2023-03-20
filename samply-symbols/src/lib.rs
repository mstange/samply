//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//!
//! You probably want to be using the [`wholesym` crate](https://docs.rs/wholesym/) instead.
//! `wholesym` has a much more ergonomic API; it is a wrapper around `samply-symbols`.
//!
//! More specifically, `samply-symbols` provides the low-level implementation of `wholesym`,
//! while satisfying both native and WebAssembly consumers, whereas `wholesym` only cares about
//! native consumers.
//!
//! The main entry point of this crate is the `SymbolManager` struct and its async `load_symbol_map` method.
//! With a `SymbolMap`, you can resolve raw code addresses to function name strings, and, if available,
//! to file name + line number information and inline stacks.
//!
//! # Design constraints
//!
//! This crate operates under the following design constraints:
//!
//!   - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
//!     WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
//!     This setup allows us to download a wasm bundle on demand, rather than shipping
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
//! by the caller. We cannot even use the `std::path::Path` / `PathBuf` types to represent paths,
//! because the WASM bundle can run on Windows, and the `Path` / `PathBuf` types have! Unix path
//! semantics in Rust-compiled-to-WebAssembly.
//!
//! Furthermore, the caller needs to be able to find the right symbol files based on a subset
//! of information about a library, for example just based on its debug name and debug ID. This
//! is used when `SymbolManager::load_symbol_map` is called with such a subset of information.
//! More concretely, this ability is used by `samply-api` when processing a JSON symbolication
//! API call, which only comes with the debug name and debug ID for a library.
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
//!     let lookup_result = symbol_map.lookup_relative_address(0x1f98f);
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
//!                         symbol_manager.lookup_external(&symbol_map.debug_file_location(), &ext_address).await
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
//!     type FL = ExampleFileLocation;
//!     type OpenFileFuture = std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     >;
//!
//!     fn get_candidate_paths_for_debug_file(
//!         &self,
//!         library_info: &LibraryInfo,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<ExampleFileLocation>>> {
//!         if let Some(debug_name) = library_info.debug_name.as_deref() {
//!             Ok(vec![CandidatePathInfo::SingleFile(ExampleFileLocation(
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
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<ExampleFileLocation>>> {
//!         if let Some(name) = library_info.name.as_deref() {
//!             Ok(vec![CandidatePathInfo::SingleFile(ExampleFileLocation(
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
//!    ) -> FileAndPathHelperResult<Vec<ExampleFileLocation>> {
//!        Ok(vec![])
//!    }
//!
//!     fn load_file(
//!         &'h self,
//!         location: ExampleFileLocation,
//!     ) -> std::pin::Pin<
//!         Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
//!     > {
//!         async fn load_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
//!             Ok(std::fs::read(&path)?)
//!         }
//!
//!         Box::pin(load_file_impl(location.0))
//!     }
//! }
//!
//! #[derive(Clone, Debug)]
//! struct ExampleFileLocation(std::path::PathBuf);
//!
//! impl std::fmt::Display for ExampleFileLocation {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         self.0.to_string_lossy().fmt(f)
//!     }
//! }
//!
//! impl FileLocation for ExampleFileLocation {
//!     fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self> {
//!         let mut filename = self.0.file_name().unwrap().to_owned();
//!         filename.push(suffix);
//!         Some(Self(self.0.with_file_name(filename)))
//!     }
//!
//!     fn location_for_external_object_file(&self, object_file: &str) -> Option<Self> {
//!         Some(Self(object_file.into()))
//!     }
//!
//!     fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self> {
//!         Some(Self(pdb_path_in_binary.into()))
//!     }
//!
//!     fn location_for_source_file(&self, source_file_path: &str) -> Option<Self> {
//!         Some(Self(source_file_path.into()))
//!     }
//!
//!     fn location_for_breakpad_symindex(&self) -> Option<Self> {
//!         Some(Self(self.0.with_extension("symindex")))
//!     }
//! }
//! ```

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
mod jitdump;
mod macho;
mod mapped_path;
mod path_mapper;
mod shared;
mod symbol_map;
mod symbol_map_object;
mod windows;

pub use crate::binary_image::{BinaryImage, CodeByteReadingError};
pub use crate::breakpad::{
    BreakpadIndex, BreakpadIndexParser, BreakpadParseError, BreakpadSymindexParseError,
};
pub use crate::cache::{FileByteSource, FileContentsWithChunkedCaching};
pub use crate::compact_symbol_table::CompactSymbolTable;
pub use crate::debugid_util::{debug_id_for_object, DebugIdExt};
pub use crate::error::Error;
pub use crate::external_file::{load_external_file, ExternalFileSymbolMap};
pub use crate::jitdump::debug_id_and_code_id_for_jitdump;
pub use crate::macho::FatArchiveMember;
pub use crate::mapped_path::MappedPath;
pub use crate::shared::{
    relative_address_base, AddressInfo, CandidatePathInfo, CodeId, ElfBuildId,
    ExternalFileAddressInFileRef, ExternalFileAddressRef, ExternalFileRef, FileAndPathHelper,
    FileAndPathHelperError, FileAndPathHelperResult, FileContents, FileContentsWrapper,
    FileLocation, FrameDebugInfo, FramesLookupResult, LibraryInfo, MultiArchDisambiguator,
    OptionallySendFuture, PeCodeId, SourceFilePath, SymbolInfo,
};
pub use crate::symbol_map::SymbolMap;

pub struct SymbolManager<'h, H: FileAndPathHelper<'h>> {
    helper: &'h H,
    cached_external_file: Mutex<Option<ExternalFileSymbolMap>>,
}

impl<'h, H, F, FL> SymbolManager<'h, H>
where
    H: FileAndPathHelper<'h, F = F, FL = FL>,
    F: FileContents + 'static,
    FL: FileLocation,
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

    pub async fn load_source_file(
        &self,
        debug_file_location: &H::FL,
        source_file_path: &SourceFilePath,
    ) -> Result<String, Error> {
        let source_file_location = debug_file_location
            .location_for_source_file(source_file_path.raw_path())
            .ok_or(Error::FileLocationRefusedSourceFileLocation)?;
        let file_contents = self
            .helper
            .load_file(source_file_location.clone())
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(source_file_location.to_string(), e))?;
        let file_contents = file_contents
            .read_bytes_at(0, file_contents.len())
            .map_err(|e| {
                Error::HelperErrorDuringFileReading(source_file_location.to_string(), e)
            })?;
        Ok(String::from_utf8_lossy(file_contents).to_string())
    }

    /// Obtain a symbol map for the library, given the (partial) `LibraryInfo`.
    /// At least the debug_id has to be given.
    pub async fn load_symbol_map(
        &self,
        library_info: &LibraryInfo,
    ) -> Result<SymbolMap<FL>, Error> {
        let debug_id = match library_info.debug_id {
            Some(debug_id) => debug_id,
            None => return Err(Error::NotEnoughInformationToIdentifySymbolMap),
        };

        let candidate_paths = self
            .helper
            .get_candidate_paths_for_debug_file(library_info)
            .map_err(|e| {
                Error::HelperErrorDuringGetCandidatePathsForDebugFile(
                    Box::new(library_info.clone()),
                    e,
                )
            })?;

        let mut last_err = None;
        for candidate_info in candidate_paths {
            let symbol_map = match candidate_info {
                CandidatePathInfo::SingleFile(file_location) => {
                    self.load_symbol_map_from_location(
                        file_location,
                        Some(MultiArchDisambiguator::DebugId(debug_id)),
                    )
                    .await
                }
                CandidatePathInfo::InDyldCache {
                    dyld_cache_path,
                    dylib_path,
                } => {
                    macho::load_symbol_map_for_dyld_cache(dyld_cache_path, dylib_path, self.helper)
                        .await
                }
            };

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
        Err(last_err
            .unwrap_or_else(|| Error::NoCandidatePathForDebugFile(Box::new(library_info.clone()))))
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
        debug_file_location: &H::FL,
        external_file_ref: &ExternalFileRef,
    ) -> Result<ExternalFileSymbolMap, Error> {
        external_file::load_external_file(self.helper, debug_file_location, external_file_ref).await
    }

    /// Resolve a debug info lookup for which `SymbolMap::lookup_*` returned a
    /// `FramesLookupResult::External`.
    ///
    /// This method is asynchronous because it may load a new external file.
    ///
    /// This keeps the most recent external file cached, so that repeated lookups
    /// for the same external file are fast.
    pub async fn lookup_external(
        &self,
        debug_file_location: &H::FL,
        address: &ExternalFileAddressRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        {
            let cached_external_file = self.cached_external_file.lock().ok()?;
            match &*cached_external_file {
                Some(external_file) if external_file.is_same_file(&address.file_ref) => {
                    return external_file.lookup(&address.address_in_file);
                }
                _ => {}
            }
        }

        let external_file = self
            .load_external_file(debug_file_location, &address.file_ref)
            .await
            .ok()?;
        let lookup_result = external_file.lookup(&address.address_in_file);

        if let Ok(mut guard) = self.cached_external_file.lock() {
            *guard = Some(external_file);
        }
        lookup_result
    }

    async fn load_binary_from_dyld_cache(
        &self,
        dyld_cache_path: FL,
        dylib_path: String,
    ) -> Result<BinaryImage<F>, Error> {
        macho::load_binary_from_dyld_cache(dyld_cache_path, dylib_path, self.helper).await
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
            let image = match candidate_info {
                CandidatePathInfo::SingleFile(file_location) => {
                    self.load_binary_at_location(
                        file_location,
                        info.name.clone(),
                        None,
                        disambiguator.clone(),
                    )
                    .await
                }
                CandidatePathInfo::InDyldCache {
                    dyld_cache_path,
                    dylib_path,
                } => {
                    self.load_binary_from_dyld_cache(dyld_cache_path, dylib_path)
                        .await
                }
            };

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

    pub async fn load_binary_for_dyld_cache_image(
        &self,
        dylib_path: &str,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<BinaryImage<F>, Error> {
        let arch = match &multi_arch_disambiguator {
            Some(MultiArchDisambiguator::Arch(arch)) => Some(arch.as_str()),
            _ => None,
        };
        let dyld_shared_cache_paths = self
            .helper
            .get_dyld_shared_cache_paths(arch)
            .map_err(Error::HelperErrorDuringGetDyldSharedCachePaths)?;

        let mut err = None;
        for dyld_cache_path in dyld_shared_cache_paths {
            let binary_res = self
                .load_binary_from_dyld_cache(dyld_cache_path, dylib_path.to_owned())
                .await;
            match (&multi_arch_disambiguator, binary_res) {
                (Some(MultiArchDisambiguator::DebugId(expected_debug_id)), Ok(binary)) => {
                    if binary.debug_id().as_ref() == Some(expected_debug_id) {
                        return Ok(binary);
                    }
                    err = Some(Error::UnmatchedDebugIdOptional(
                        *expected_debug_id,
                        binary.debug_id(),
                    ));
                }
                (_, Ok(binary)) => return Ok(binary),
                (_, Err(e)) => err = Some(e),
            }
        }
        Err(err.unwrap_or(Error::NoCandidatePathForDyldCache))
    }

    pub async fn load_symbol_map_for_dyld_cache_image(
        &self,
        dylib_path: &str,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap<FL>, Error> {
        let arch = match &multi_arch_disambiguator {
            Some(MultiArchDisambiguator::Arch(arch)) => Some(arch.as_str()),
            _ => None,
        };
        let dyld_shared_cache_paths = self
            .helper
            .get_dyld_shared_cache_paths(arch)
            .map_err(Error::HelperErrorDuringGetDyldSharedCachePaths)?;

        let mut err = None;
        for dyld_cache_path in dyld_shared_cache_paths {
            let symbol_map_res = macho::load_symbol_map_for_dyld_cache(
                dyld_cache_path,
                dylib_path.to_owned(),
                self.helper,
            )
            .await;
            match (&multi_arch_disambiguator, symbol_map_res) {
                (Some(MultiArchDisambiguator::DebugId(expected_debug_id)), Ok(symbol_map)) => {
                    if &symbol_map.debug_id() == expected_debug_id {
                        return Ok(symbol_map);
                    }
                    err = Some(Error::UnmatchedDebugId(
                        symbol_map.debug_id(),
                        *expected_debug_id,
                    ));
                }
                (_, Ok(symbol_map)) => return Ok(symbol_map),
                (_, Err(e)) => err = Some(e),
            }
        }
        Err(err.unwrap_or(Error::NoCandidatePathForDyldCache))
    }

    pub async fn load_symbol_map_from_location(
        &self,
        file_location: FL,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap<FL>, Error> {
        let file_contents = self
            .helper
            .load_file(file_location.clone())
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(file_location.to_string(), e))?;

        let file_contents = FileContentsWrapper::new(file_contents);

        if let Ok(file_kind) = FileKind::parse(&file_contents) {
            match file_kind {
                FileKind::Elf32 | FileKind::Elf64 => {
                    elf::load_symbol_map_for_elf(
                        file_location,
                        file_contents,
                        file_kind,
                        self.helper,
                    )
                    .await
                }
                FileKind::MachOFat32 | FileKind::MachOFat64 => {
                    let member = macho::get_fat_archive_member(
                        &file_contents,
                        file_kind,
                        multi_arch_disambiguator,
                    )?;
                    macho::get_symbol_map_for_fat_archive_member(
                        file_location,
                        file_contents,
                        member,
                    )
                }
                FileKind::MachO32 | FileKind::MachO64 => {
                    macho::get_symbol_map_for_macho(file_location, file_contents)
                }
                FileKind::Pe32 | FileKind::Pe64 => {
                    match windows::load_symbol_map_for_pdb_corresponding_to_binary(
                        file_kind,
                        &file_contents,
                        file_location.clone(),
                        self.helper,
                    )
                    .await
                    {
                        Ok(symbol_map) => Ok(symbol_map),
                        Err(_) => {
                            windows::get_symbol_map_for_pe(file_contents, file_kind, file_location)
                        }
                    }
                }
                _ => Err(Error::InvalidInputError(
                    "Input was Archive, Coff or Wasm format, which are unsupported for now",
                )),
            }
        } else if windows::is_pdb_file(&file_contents) {
            windows::get_symbol_map_for_pdb(file_contents, file_location)
        } else if breakpad::is_breakpad_file(&file_contents) {
            let index_file_contents =
                if let Some(index_file_location) = file_location.location_for_breakpad_symindex() {
                    self.helper
                        .load_file(index_file_location)
                        .await
                        .ok()
                        .map(FileContentsWrapper::new)
                } else {
                    None
                };
            breakpad::get_symbol_map_for_breakpad_sym(
                file_contents,
                file_location,
                index_file_contents,
            )
        } else if jitdump::is_jitdump_file(&file_contents) {
            jitdump::get_symbol_map_for_jitdump(file_contents, file_location)
        } else {
            Err(Error::InvalidInputError(
            "The file does not have a known format; PDB::open was not able to parse it and object::FileKind::parse was not able to detect the format.",
        ))
        }
    }

    pub async fn load_binary_at_location(
        &self,
        file_location: H::FL,
        name: Option<String>,
        path: Option<String>,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<BinaryImage<F>, Error> {
        let file_contents = self
            .helper
            .load_file(file_location.clone())
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(file_location.to_string(), e))?;

        let file_contents = FileContentsWrapper::new(file_contents);

        let file_kind = match FileKind::parse(&file_contents) {
            Ok(file_kind) => file_kind,
            Err(_) if jitdump::is_jitdump_file(&file_contents) => {
                let inner = BinaryImageInner::JitDump(file_contents);
                return BinaryImage::new(inner, name, path);
            }
            Err(_) => {
                return Err(Error::InvalidInputError("Unrecognized file"));
            }
        };
        let inner = match file_kind {
            FileKind::Elf32
            | FileKind::Elf64
            | FileKind::MachO32
            | FileKind::MachO64
            | FileKind::Pe32
            | FileKind::Pe64 => BinaryImageInner::Normal(file_contents, file_kind),
            FileKind::MachOFat32 | FileKind::MachOFat64 => {
                let member = macho::get_fat_archive_member(
                    &file_contents,
                    file_kind,
                    multi_arch_disambiguator,
                )?;
                let (offset, size) = member.offset_and_size;
                let arch = member.arch;
                let data = macho::MachOFatArchiveMemberData::new(file_contents, offset, size, arch);
                BinaryImageInner::MemberOfFatArchive(data, file_kind)
            }
            _ => {
                return Err(Error::InvalidInputError(
                    "Input was Archive, Coff or Wasm format, which are unsupported for now",
                ))
            }
        };
        BinaryImage::new(inner, name, path)
    }
}
