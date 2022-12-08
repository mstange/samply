//! This crate implements a JSON API for profiler symbolication with the help of
//! local symbol files. It exposes a single function `query_api`, and uses the
//! `samply-symbols` crate for its implementation.
//!
//! The API is documented in [API.md](../API.md).
//!
//! Just like the `samply-symbols` crate, this crate does not contain any direct
//! file access. It is written in such a way that it can be compiled to
//! WebAssembly, with all file access being mediated via a `FileAndPathHelper`
//! trait.
//!
//! ## Example
//!
//! ```rust
//! use samply_api::samply_symbols::{
//!     FileContents, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture,
//!     CandidatePathInfo, FileLocation, SymbolManager,
//! };
//! use samply_api::samply_symbols::debugid::{CodeId, DebugId};
//!
//! async fn run_query() -> String {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
//!     };
//!     let symbol_manager = SymbolManager::with_helper(&helper);
//!     let api = samply_api::Api::new(&symbol_manager);
//!
//!     api.query_api(
//!         "/symbolicate/v5",
//!         r#"{
//!             "memoryMap": [
//!               [
//!                 "firefox.pdb",
//!                 "AA152DEB2D9B76084C4C44205044422E1"
//!               ]
//!             ],
//!             "stacks": [
//!               [
//!                 [0, 204776],
//!                 [0, 129423],
//!                 [0, 244290],
//!                 [0, 244219]
//!               ]
//!             ]
//!           }"#,
//!     ).await
//! }
//!
//! struct ExampleHelper {
//!     artifact_directory: std::path::PathBuf,
//! }
//!
//! impl<'h> FileAndPathHelper<'h> for ExampleHelper {
//!     type F = Vec<u8>;
//!     type OpenFileFuture =
//!         std::pin::Pin<Box<dyn std::future::Future<Output = FileAndPathHelperResult<Self::F>> + 'h>>;
//!
//!     fn get_candidate_paths_for_debug_file(
//!         &self,
//!         debug_name: &str,
//!         _debug_id: DebugId,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(self.artifact_directory.join(debug_name)))])
//!     }
//!
//!     fn get_candidate_paths_for_binary(
//!         &self,
//!         _debug_name: Option<&str>,
//!         _debug_id: Option<DebugId>,
//!         name: Option<&str>,
//!         _code_id: Option<&CodeId>,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         if let Some(name) = name {
//!             Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(
//!                 self.artifact_directory.join(name),
//!             ))])
//!         } else {
//!             Ok(vec![])
//!         }
//!     }
//!
//!     fn open_file(
//!         &'h self,
//!         location: &FileLocation,
//!     ) -> std::pin::Pin<Box<dyn std::future::Future<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
//!         async fn read_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
//!             Ok(std::fs::read(&path)?)
//!         }
//!
//!         let path = match location {
//!             FileLocation::Path(path) => path.clone(),
//!             FileLocation::Custom(_) => panic!("Unexpected FileLocation::Custom"),
//!         };
//!         Box::pin(read_file_impl(path.to_path_buf()))
//!     }
//! }
//! ```

pub use samply_symbols;
pub use samply_symbols::debugid;
use samply_symbols::{FileAndPathHelper, SymbolManager};

use debugid::DebugId;
use serde_json::json;
use source::SourceApi;
use symbolicate::SymbolicateApi;

mod error;
mod source;
mod symbolicate;

pub(crate) fn to_debug_id(breakpad_id: &str) -> Result<DebugId, samply_symbols::Error> {
    DebugId::from_breakpad(breakpad_id)
        .map_err(|_| samply_symbols::Error::InvalidBreakpadId(breakpad_id.to_string()))
}

#[derive(Clone, Copy)]
pub struct Api<'a, 'h: 'a, H: FileAndPathHelper<'h>> {
    symbol_manager: &'a SymbolManager<'h, H>,
}

impl<'a, 'h: 'a, H: FileAndPathHelper<'h>> Api<'a, 'h, H> {
    /// Create a [`Api`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<'h, H>) -> Self {
        Self { symbol_manager }
    }

    /// This is the main API of this crate.
    /// It implements the "Tecken" JSON API, which is also used by the Mozilla symbol server.
    /// It's intended to be used as a drop-in "local symbol server" which gathers its data
    /// directly from file artifacts produced during compilation (rather than consulting
    /// e.g. a database).
    /// The caller needs to implement the `FileAndPathHelper` trait to provide file system access.
    /// The return value is a JSON string.
    ///
    /// The following "URLs" are supported:
    ///  - `/symbolicate/v5`: This API is documented at <https://tecken.readthedocs.io/en/latest/symbolication.html>.
    ///    The returned data has two extra fields: inlines (per address) and module_errors (per job).
    ///  - `/source/v1`: Experimental API. Symbolicates an address and lets you read one of the files in the
    ///    symbol information for that address.
    pub async fn query_api(self, request_url: &str, request_json_data: &str) -> String {
        if request_url == "/symbolicate/v5" {
            let symbolicate_api = SymbolicateApi::new(self.symbol_manager);
            symbolicate_api.query_api_json(request_json_data).await
        } else if request_url == "/source/v1" {
            let source_api = SourceApi::new(self.symbol_manager);
            source_api.query_api_json(request_json_data).await
        } else {
            json!({ "error": format!("Unrecognized URL {}", request_url) }).to_string()
        }
    }
}

#[cfg(all(test, feature = "send_futures"))]
mod test {
    use crate::debugid::DebugId;
    use crate::{
        AddressDebugInfo, CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult,
        FileLocation, OptionallySendFuture, SymbolicationQuery, SymbolicationResult,
    };

    #[allow(unused)]
    fn test_send() {
        struct TestSendHelper;

        impl<'h> FileAndPathHelper<'h> for TestSendHelper {
            type F = Vec<u8>;
            type OpenFileFuture = std::pin::Pin<
                Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
            >;
            fn get_candidate_paths_for_debug_file(
                &self,
                debug_name: &str,
                _debug_id: &DebugId,
            ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
                panic!()
            }
            fn open_file(
                &'h self,
                location: &FileLocation,
            ) -> std::pin::Pin<
                Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
            > {
                panic!()
            }
        }

        let helper = TestSendHelper;
        let query: SymbolicationQuery = panic!();
        let f = crate::query_api("/symbolicate/v5", "{}", &helper);

        fn assert_send<T: Send>(_x: T) {}
        assert_send(f);
    }
}
