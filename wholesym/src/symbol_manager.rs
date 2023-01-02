use std::borrow::Cow;
use std::path::Path;
use std::{future::Future, pin::Pin};

use debugid::DebugId;
use samply_api::samply_symbols::{
    self, AddressInfo, Error, ExternalFileAddressRef, ExternalFileRef, ExternalFileSymbolMap,
    InlineStackFrame, LibraryInfo, MultiArchDisambiguator,
};
use samply_api::Api;
use yoke::{Yoke, Yokeable};

use crate::config::SymbolManagerConfig;
use crate::helper::{FileReadOnlyHelper, Helper, WholesymFileLocation};

pub struct SymbolMap(samply_api::samply_symbols::SymbolMap<WholesymFileLocation>);

impl SymbolMap {
    pub fn debug_id(&self) -> debugid::DebugId {
        self.0.debug_id()
    }

    pub fn symbol_count(&self) -> usize {
        self.0.symbol_count()
    }

    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.iter_symbols()
    }

    pub fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.lookup(address)
    }
}

pub struct SymbolManager {
    helper_with_symbol_manager: Yoke<SymbolManagerWrapperTypeErased<'static>, Box<Helper>>,
}

impl SymbolManager {
    /// Create a new `SymbolManager` with the given config.
    pub fn with_config(config: SymbolManagerConfig) -> Self {
        let helper = Helper::with_config(config);
        let helper_with_symbol_manager = Yoke::attach_to_cart(Box::new(helper), |helper| {
            let symbol_manager = samply_symbols::SymbolManager::with_helper(helper);
            SymbolManagerWrapperTypeErased(Box::new(SymbolManagerWrapper(symbol_manager)))
        });
        Self {
            helper_with_symbol_manager,
        }
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
        self.helper_with_symbol_manager
            .get()
            .0
            .load_symbol_map_for_binary_at_path(path, disambiguator)
            .await
    }

    /// Computes the `LibraryInfo` for the given binary. This `LibraryInfo`
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
        let symbol_manager = samply_symbols::SymbolManager::with_helper(&helper);
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
        self.helper_with_symbol_manager
            .with_mut(|manager| manager.0.add_known_lib(lib_info));
    }

    /// Obtain a symbol map for the given `debug_name` and `debug_id`.
    pub async fn load_symbol_map(
        &self,
        debug_name: &str,
        debug_id: DebugId,
    ) -> Result<SymbolMap, Error> {
        self.helper_with_symbol_manager
            .get()
            .0
            .load_symbol_map(debug_name, debug_id)
            .await
    }

    /// Resolve a debug info lookup for which `SymbolMap::lookup` returned a
    /// `FramesLookupResult::External`.
    ///
    /// This method is asynchronous because it may load a new external file.
    ///
    /// This is used on macOS: When linking multiple `.o` files together into a library or
    /// an executable, the linker does not copy the dwarf sections into the linked output.
    /// Instead, it stores the paths to those original `.o` files, using OSO stabs entries.
    ///
    /// A `SymbolMap` for such a linked file will not contain debug info, and will return
    /// `FramesLookupResult::External` from the lookups. Then the address needs to be
    /// looked up in the external file.
    ///
    /// The `SymbolManager` keeps the most recent external file cached, so that repeated
    /// calls to `lookup_external` for the same external file are fast. If the set of
    /// addresses for lookup is known ahead-of-time, sorting these addresses upfront can
    /// achieve a very good hit rate.
    pub async fn lookup_external(
        &self,
        symbol_map: &SymbolMap,
        address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        self.helper_with_symbol_manager
            .get()
            .0
            .lookup_external(symbol_map, address)
            .await
    }

    /// Manually load and return an external file with additional debug info.
    /// This is a lower-level alternative to [`lookup_external`](SymbolManager::lookup_external)
    /// and can be used if more control over caching is desired.
    pub async fn load_external_file(
        &self,
        symbol_map: &SymbolMap,
        external_file_ref: &ExternalFileRef,
    ) -> Result<ExternalFileSymbolMap, Error> {
        self.helper_with_symbol_manager
            .get()
            .0
            .load_external_file(symbol_map, external_file_ref)
            .await
    }

    /// Run a symbolication query with the "Tecken" JSON API.
    ///
    /// In the future, this will be a feature on this crate and not enabled by default.
    pub async fn query_json_api(&self, path: &str, request_json: &str) -> String {
        self.helper_with_symbol_manager
            .get()
            .0
            .query_json_api(path, request_json)
            .await
    }
}

// Do a trait dance to create a covariant wrapper.
// This is necessary because samply_symbols::SymbolManager has a generic parameter
// `H: FileAndPathHelper<'h>` - the lifetime is named in the trait. And *that's*
// only necessary because FileAndPathHelper's OpenFileFuture needs a lifetime
// parameter. Starting with Rust 1.65, OpenFileFuture could use GATs for the lifetime
// parameter, but we're keeping a lower MSRV for now. Maybe we can revisit this mid-2023.
#[derive(Yokeable)]
struct SymbolManagerWrapperTypeErased<'h>(Box<dyn SymbolManagerTrait + 'h + Send + Sync>);

trait SymbolManagerTrait {
    fn add_known_lib(&mut self, lib_info: LibraryInfo);

    fn load_symbol_map<'a>(
        &'a self,
        debug_name: &'a str,
        debug_id: DebugId,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>>;

    fn load_symbol_map_for_binary_at_path<'a>(
        &'a self,
        path: &'a Path,
        disambiguator: Option<MultiArchDisambiguator>,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>>;

    fn lookup_external<'a>(
        &'a self,
        symbol_map: &'a SymbolMap,
        address: &'a ExternalFileAddressRef,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<InlineStackFrame>>> + 'a + Send>>;

    fn load_external_file<'a>(
        &'a self,
        symbol_map: &'a SymbolMap,
        external_file_ref: &'a ExternalFileRef,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalFileSymbolMap, Error>> + 'a + Send>>;

    fn query_json_api<'a>(
        &'a self,
        path: &'a str,
        request_json: &'a str,
    ) -> Pin<Box<dyn Future<Output = String> + 'a + Send>>;
}

struct SymbolManagerWrapper<'h>(samply_symbols::SymbolManager<'h, Helper>);

impl<'h> SymbolManagerWrapper<'h> {
    async fn load_symbol_map_for_binary_at_path_impl(
        &self,
        path: &Path,
        multi_arch_disambiguator: Option<MultiArchDisambiguator>,
    ) -> Result<SymbolMap, Error> {
        let library_info =
            SymbolManager::library_info_for_binary_at_path(path, multi_arch_disambiguator).await?;
        self.load_symbol_map(library_info).await
    }

    async fn load_symbol_map(&self, info: LibraryInfo) -> Result<SymbolMap, Error> {
        Ok(SymbolMap(self.0.load_symbol_map(&info).await?))
    }
}

impl<'h> SymbolManagerTrait for SymbolManagerWrapper<'h> {
    fn add_known_lib(&mut self, lib_info: LibraryInfo) {
        self.0.helper().add_known_lib(lib_info);
    }

    fn load_symbol_map<'a>(
        &'a self,
        debug_name: &'a str,
        debug_id: DebugId,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>> {
        let info = LibraryInfo {
            debug_name: Some(debug_name.to_string()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        Box::pin(self.load_symbol_map(info))
    }

    fn load_symbol_map_for_binary_at_path<'a>(
        &'a self,
        path: &'a Path,
        disambiguator: Option<MultiArchDisambiguator>,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>> {
        Box::pin(self.load_symbol_map_for_binary_at_path_impl(path, disambiguator))
    }

    fn lookup_external<'a>(
        &'a self,
        symbol_map: &'a SymbolMap,
        address: &'a ExternalFileAddressRef,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<InlineStackFrame>>> + 'a + Send>> {
        Box::pin(
            self.0
                .lookup_external(symbol_map.0.debug_file_location(), address),
        )
    }

    fn load_external_file<'a>(
        &'a self,
        symbol_map: &'a SymbolMap,
        external_file_ref: &'a ExternalFileRef,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalFileSymbolMap, Error>> + 'a + Send>> {
        Box::pin(
            self.0
                .load_external_file(symbol_map.0.debug_file_location(), external_file_ref),
        )
    }

    fn query_json_api<'a>(
        &'a self,
        path: &'a str,
        request_json: &'a str,
    ) -> Pin<Box<dyn Future<Output = String> + 'a + Send>> {
        let api = Api::new(&self.0);
        let f = api.query_api(path, request_json);
        Box::pin(f)
    }
}
