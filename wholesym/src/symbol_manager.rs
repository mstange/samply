use std::{future::Future, pin::Pin};

use debugid::DebugId;
use samply_api::{
    samply_symbols::{self, ExternalFileSymbolMap},
    Api,
};
use samply_symbols::{Error, ExternalFileAddressRef, ExternalFileRef, InlineStackFrame, SymbolMap};
use yoke::{Yoke, Yokeable};

use crate::{helper::Helper, SymbolManagerConfig};

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

    /// Obtain a symbol map for the given `debug_name` and `debug_id`.
    pub async fn get_symbol_map(
        &self,
        debug_name: &str,
        debug_id: DebugId,
    ) -> Result<SymbolMap, Error> {
        self.helper_with_symbol_manager
            .get()
            .0
            .get_symbol_map(debug_name, debug_id)
            .await
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
        external_file_ref: &ExternalFileRef,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        self.helper_with_symbol_manager
            .get()
            .0
            .lookup_external(external_file_ref, external_file_address)
            .await
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
    pub async fn get_external_file(
        &self,
        external_file_ref: &ExternalFileRef,
    ) -> Result<ExternalFileSymbolMap, Error> {
        self.helper_with_symbol_manager
            .get()
            .0
            .get_external_file(external_file_ref)
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
    fn get_symbol_map<'a>(
        &'a self,
        debug_name: &'a str,
        debug_id: DebugId,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>>;

    fn lookup_external<'a>(
        &'a self,
        external_file_ref: &'a ExternalFileRef,
        external_file_address: &'a ExternalFileAddressRef,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<InlineStackFrame>>> + 'a + Send>>;

    fn get_external_file<'a>(
        &'a self,
        external_file_ref: &'a ExternalFileRef,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalFileSymbolMap, Error>> + 'a + Send>>;

    fn query_json_api<'a>(
        &'a self,
        path: &'a str,
        request_json: &'a str,
    ) -> Pin<Box<dyn Future<Output = String> + 'a + Send>>;
}

struct SymbolManagerWrapper<'h>(samply_symbols::SymbolManager<'h, Helper>);

impl<'h> SymbolManagerTrait for SymbolManagerWrapper<'h> {
    fn get_symbol_map<'a>(
        &'a self,
        debug_name: &'a str,
        debug_id: DebugId,
    ) -> Pin<Box<dyn Future<Output = Result<SymbolMap, Error>> + 'a + Send>> {
        Box::pin(self.0.get_symbol_map(debug_name, debug_id))
    }

    fn lookup_external<'a>(
        &'a self,
        external_file_ref: &'a ExternalFileRef,
        external_file_address: &'a ExternalFileAddressRef,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<InlineStackFrame>>> + 'a + Send>> {
        Box::pin(
            self.0
                .lookup_external(external_file_ref, external_file_address),
        )
    }

    fn get_external_file<'a>(
        &'a self,
        external_file_ref: &'a ExternalFileRef,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalFileSymbolMap, Error>> + 'a + Send>> {
        Box::pin(self.0.get_external_file(external_file_ref))
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
