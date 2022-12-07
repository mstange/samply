use std::{future::Future, pin::Pin};

use debugid::DebugId;
use samply_api::{samply_symbols, Api};
use samply_symbols::{Error, ExternalFileAddressRef, ExternalFileRef, InlineStackFrame, SymbolMap};
use yoke::{Yoke, Yokeable};

use crate::{helper::Helper, SymbolicatorConfig};

pub struct Symbolicator {
    helper_with_symbolicator: Yoke<SymbolicatorWrapperTypeErased<'static>, Box<Helper>>,
}

impl Symbolicator {
    /// Create a new `Symbolicator` with the given config.
    pub fn with_config(config: SymbolicatorConfig) -> Self {
        let helper = Helper::with_config(config);
        let helper_with_symbolicator = Yoke::attach_to_cart(Box::new(helper), |helper| {
            let symbolicator = samply_symbols::Symbolicator::with_helper(helper);
            SymbolicatorWrapperTypeErased(Box::new(SymbolicatorWrapper(symbolicator)))
        });
        Self {
            helper_with_symbolicator,
        }
    }

    /// Obtain a symbol map for the given `debug_name` and `debug_id`.
    pub async fn get_symbol_map(
        &self,
        debug_name: &str,
        debug_id: DebugId,
    ) -> Result<SymbolMap, Error> {
        self.helper_with_symbolicator
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
        self.helper_with_symbolicator
            .get()
            .0
            .lookup_external(external_file_ref, external_file_address)
            .await
    }

    /// Run a symbolication query with the "Tecken" JSON API.
    ///
    /// In the future, this will be a feature on this crate and not enabled by default.
    pub async fn query_json_api(&self, path: &str, request_json: &str) -> String {
        self.helper_with_symbolicator
            .get()
            .0
            .query_json_api(path, request_json)
            .await
    }
}

// Do a trait dance to create a covariant wrapper.
// This is necessary because samply_symbols::Symbolicator has a generic parameter
// `H: FileAndPathHelper<'h>` - the lifetime is named in the trait. And *that's*
// only necessary because FileAndPathHelper's OpenFileFuture needs a lifetime
// parameter. Starting with Rust 1.65, OpenFileFuture could use GATs for the lifetime
// parameter, but we're keeping a lower MSRV for now. Maybe we can revisit this mid-2023.
#[derive(Yokeable)]
struct SymbolicatorWrapperTypeErased<'h>(Box<dyn SymbolicatorTrait + 'h + Send + Sync>);

trait SymbolicatorTrait {
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

    fn query_json_api<'a>(
        &'a self,
        path: &'a str,
        request_json: &'a str,
    ) -> Pin<Box<dyn Future<Output = String> + 'a + Send>>;
}

struct SymbolicatorWrapper<'h>(samply_symbols::Symbolicator<'h, Helper>);

impl<'h> SymbolicatorTrait for SymbolicatorWrapper<'h> {
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
