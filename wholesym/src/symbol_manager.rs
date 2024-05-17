use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use debugid::DebugId;
use samply_symbols::{
    self, AddressInfo, Error, ExternalFileAddressInFileRef, ExternalFileAddressRef, FrameDebugInfo,
    LibraryInfo, LookupAddress, MultiArchDisambiguator, SymbolMapTrait, SyncAddressInfo,
};

use crate::config::SymbolManagerConfig;
use crate::helper::{FileReadOnlyHelper, Helper, WholesymFileContents, WholesymFileLocation};

/// Used in [`SymbolManager::load_external_file`] and returned by [`SymbolMap::symbol_file_origin`].
#[derive(Debug, Clone)]
pub struct SymbolFileOrigin(WholesymFileLocation);

/// Contains the symbols for a binary, and allows querying them by address and iterating over them.
///
/// Symbols can be looked up by three types of addresses:
///
///  - Relative addresses, i.e. `u32` addresses which are relative to the image base address.
///  - SVMAs, or "stated virtual memory addresses", i.e. `u64` addresses which are meaningful
///    in the virtual memory space defined by the binary file. Symbol addresess and section
///    addresses in the binary are in this space.
///  - File offsets into the binary file. These are used when you have an absolute address in
///    the virtual memory of a running process, and map it to a file offset with the help
///    of process maps, e.g. with the help of `/proc/<pid>/maps` on Linux.
///
/// Sometimes it can be easy to mix these address types up, especially if you're testing with
/// a file for which all three are the same. For a file for which all three are different,
/// check out [this `firefox` binary](https://github.com/mstange/samply/blob/841f97b0df3ecefddf8f9ba2b7d39fdcc79a79f5/fixtures/linux64-ci/firefox),
/// which has the following ELF LOAD commands ("segments"):
///
/// ```plain
/// Type           Offset   VirtAddr           PhysAddr           FileSiz  MemSiz   Flg Align
/// LOAD           0x000000 0x0000000000200000 0x0000000000200000 0x000894 0x000894 R   0x1000
/// LOAD           0x0008a0 0x00000000002018a0 0x00000000002018a0 0x0002f0 0x0002f0 R E 0x1000
/// LOAD           0x000b90 0x0000000000202b90 0x0000000000202b90 0x000200 0x000200 RW  0x1000
/// LOAD           0x000d90 0x0000000000203d90 0x0000000000203d90 0x000068 0x000069 RW  0x1000
/// ```
///
/// For example, in this file, the file offset `0x9ea` falls into the second segment and
/// corresponds to the relative address `0x19ea` and to the SVMA `0x2019ea`.
/// The image base address is `0x200000`.
pub struct SymbolMap(samply_symbols::SymbolMap<Helper>);

impl SymbolMap {
    /// Look up symbol information for the specified [`LookupAddress`].
    ///
    /// This method is asynchronous because it might need to load additional files,
    /// for example `.dwo` files on Linux or `.o` files on macOS. You can use
    /// [`SymbolMap::lookup_sync`] if you're calling this in a context where
    /// you cannot await, or if you don't care about the `.dwo` / `.o` cases.
    pub async fn lookup(&self, address: LookupAddress) -> Option<AddressInfo> {
        self.0.lookup(address).await
    }

    /// Look up symbol information, using only files that have already been loaded.
    ///
    /// If additional files are needed to fully resolve the frame information, this
    /// will be indicated in the returned [`SyncAddressInfo`]: [`SyncAddressInfo::frames`]
    /// will be `Some(FramesLookupResult::External(...))`.
    /// Then you can use [`SymbolMap::lookup_external`] to resolve lookups for these addresses.
    ///
    /// Usually you would just use [`SymbolMap::lookup`] instead of `lookup_sync` +
    /// `lookup_external`. However, there exists a case where doing it manually can perform
    /// better: If you have many addresses you need to batch-lookup, and you're on macOS
    /// where some addresses will have to be resolved by loading `.o` files, then you can
    /// reorder the `lookup_external` calls and avoid reloading the same `.o` file
    /// multiple times. You'd do this by doing the lookup in two passes: First, call
    /// `lookup_sync` for every address, and collect all the `ExternalFileAddressRef`s.
    /// Then, sort the collected `ExternalFileAddressRef`s. This will make it so that all
    /// addresses that need the same `.o` file are grouped together. Then, call
    /// `lookup_external` for each `ExternalFileAddressRef` in the sorted order.
    /// The `SymbolMap` only caches a single `.o` file at a time.
    pub fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        self.0.lookup_sync(address)
    }

    /// Resolve a debug info lookup for which `SymbolMap::lookup_*` returned
    /// [`FramesLookupResult::External`](crate::FramesLookupResult::External).
    ///
    /// This method is asynchronous because it may load a new external file.
    ///
    /// This is used on macOS and on Linux with "unpacked" debuginfo: On macOS it is used
    /// whenever there is no dSYM, and on Linux it is used to support `-gsplit-dwarf`.
    /// The debug info is obtained from `.o`/`.a` and `.dwo` files, respectively.
    ///
    /// For the macOS case, the `SymbolMap` keeps the most recent external file cached,
    /// so that repeated calls to `lookup_external` for the same external file are fast.
    /// For the Linux `.dwo` case, the `SymbolMap` accumulates all `.dwo` files that have
    /// been loaded for `lookup_external` calls.
    pub async fn lookup_external(
        &self,
        external: &ExternalFileAddressRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        self.0.lookup_external(external).await
    }

    /// Returns an abstract "origin token" which can be passed to [`SymbolManager::load_external_file`]
    /// when resolving [`FramesLookupResult::External`](crate::FramesLookupResult::External) addresses.
    ///
    /// Internally, this is used to ensure that we don't follow random paths found in symbol
    /// files which were downloaded from a symbol server - we only want to load files from
    /// these paths if the file that contained these paths is also a local file.
    pub fn symbol_file_origin(&self) -> SymbolFileOrigin {
        SymbolFileOrigin(self.0.debug_file_location().clone())
    }

    /// The Debug ID of the binary that is described by the symbol information in this `SymbolMap`.
    pub fn debug_id(&self) -> debugid::DebugId {
        self.0.debug_id()
    }

    /// The number of symbols (usually function entries) in this `SymbolMap`.
    pub fn symbol_count(&self) -> usize {
        self.0.symbol_count()
    }

    /// Iterate over all symbols in this `SymbolMap`.
    ///
    /// This iterator yields the relative address and the name of each symbol.
    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.iter_symbols()
    }
}

pub struct ExternalFileSymbolMap(samply_symbols::ExternalFileSymbolMap<WholesymFileContents>);

impl ExternalFileSymbolMap {
    /// The string which identifies this external file. This is usually an absolute
    /// path.
    pub fn file_path(&self) -> &str {
        self.0.file_path()
    }

    /// Look up the debug info for the given [`ExternalFileAddressInFileRef`].
    pub fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        self.0.lookup(external_file_address)
    }
}

/// Allows obtaining [`SymbolMap`]s.
pub struct SymbolManager {
    symbol_manager: samply_symbols::SymbolManager<Helper>,
}

impl SymbolManager {
    /// Create a new `SymbolManager` with the given config.
    pub fn with_config(config: SymbolManagerConfig) -> Self {
        let helper = Helper::with_config(config);
        let symbol_manager = samply_symbols::SymbolManager::with_helper(helper);
        Self { symbol_manager }
    }

    /// Find symbols for the given binary.
    ///
    /// On macOS, the given path can also be a path to a system library which is
    /// stored in the dyld shared cache.
    ///
    /// The `disambiguator` is only used on macOS, for picking the right member of
    /// a universal binary ("fat archive"), or for picking the right dyld shared cache.
    /// On other platforms, `disambiguator` can be set to `None`.
    pub async fn load_symbol_map_for_binary_at_path(
        &self,
        path: &Path,
        disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap, Error> {
        let library_info = Self::library_info_for_binary_at_path(path, disambiguator).await?;

        Ok(SymbolMap(
            self.symbol_manager.load_symbol_map(&library_info).await?,
        ))
    }

    /// Computes the [`LibraryInfo`] for the given binary. This [`LibraryInfo`]
    /// can be stored and used to identify symbol data for this binary at a later
    /// time.
    ///
    /// For example, on Windows, this computes the code ID, debug ID and PDB name
    /// for this binary, allowing both the binary and the debug info to be obtained
    /// from a Windows symbol server at a later time.
    ///
    /// On Linux and macOS, this reads the ELF build ID / mach-O UUID, which can
    /// also be used to identify the correct debug file later, or to obtain such a
    /// file from a server (e.g. debuginfod for Linux).
    pub async fn library_info_for_binary_at_path(
        path: &Path,
        disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<LibraryInfo, Error> {
        let might_be_in_dyld_shared_cache =
            path.starts_with("/usr/") || path.starts_with("/System/");

        let helper = FileReadOnlyHelper;
        let symbol_manager = samply_symbols::SymbolManager::with_helper(helper);
        let name = path
            .file_name()
            .and_then(|name| Some(name.to_str()?.to_owned()));
        let path_str = path.to_str().map(ToOwned::to_owned);
        let binary_res = symbol_manager
            .load_binary_at_location(
                WholesymFileLocation::LocalFile(path.to_owned()),
                name,
                path_str,
                disambiguator.clone(),
            )
            .await;
        let binary = match binary_res {
            Ok(binary) => binary,
            Err(Error::HelperErrorDuringOpenFile(_, _)) if might_be_in_dyld_shared_cache => {
                // The file at the given path could not be opened, so it probably doesn't exist.
                // Check the dyld cache.
                symbol_manager
                    .load_binary_for_dyld_cache_image(&path.to_string_lossy(), disambiguator)
                    .await?
            }
            Err(e) => return Err(e),
        };
        Ok(binary.library_info())
    }

    /// Tell the `SymbolManager` about a known library. This allows it to find
    /// debug files or binaries later based on a subset of the library information.
    ///
    /// This is mostly used to make [`query_json_api`](SymbolManager::query_json_api)
    /// work properly: The JSON request for symbols only contains
    /// `(debug_name, debug_id)` pairs, so there needs to be some stored auxiliary
    /// information which allows us to find the right debug files for the request.
    /// The list of "known libraries" is this auxiliary information.
    pub fn add_known_library(&mut self, lib_info: LibraryInfo) {
        self.symbol_manager.helper().add_known_lib(lib_info);
    }

    /// Tell the `SymbolManager` about a library's symbol table. The library
    /// must contain a DebugId. This is useful when a library's symbols are
    /// available in some way other than normal symbol lookup, or if a custom
    /// format is desired that is not natively supported.
    pub fn add_known_library_symbols(
        &mut self,
        lib_info: LibraryInfo,
        symbol_map: Arc<dyn SymbolMapTrait + Send + Sync>,
    ) {
        self.symbol_manager
            .helper()
            .add_precog_symbol_map(lib_info, symbol_map);
    }

    /// Obtain a symbol map for the given `debug_name` and `debug_id`.
    pub async fn load_symbol_map(
        &self,
        debug_name: &str,
        debug_id: DebugId,
    ) -> Result<SymbolMap, Error> {
        let info = LibraryInfo {
            debug_name: Some(debug_name.to_string()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        Ok(SymbolMap(self.symbol_manager.load_symbol_map(&info).await?))
    }

    /// Manually load and return an external file with additional debug info.
    /// This is a lower-level alternative to [`lookup_external`](SymbolMap::lookup_external)
    /// and can be used if more control over caching is desired.
    pub async fn load_external_file(
        &self,
        symbol_file_origin: &SymbolFileOrigin,
        external_file_path: &str,
    ) -> Result<ExternalFileSymbolMap, Error> {
        let symbol_map = self
            .symbol_manager
            .load_external_file(&symbol_file_origin.0, external_file_path)
            .await?;
        Ok(ExternalFileSymbolMap(symbol_map))
    }

    /// Run a symbolication query with the "Tecken" JSON API.
    #[cfg(feature = "api")]
    pub async fn query_json_api(&self, path: &str, request_json: &str) -> String {
        let api = samply_api::Api::new(&self.symbol_manager);
        api.query_api(path, request_json).await
    }
}
