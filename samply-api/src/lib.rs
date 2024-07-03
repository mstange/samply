//! This crate implements a JSON API for profiler symbolication with the help of
//! local symbol files. It exposes a single type called `API`, and uses the
//! `samply-symbols` crate for its implementation.
//!
//! Just like the `samply-symbols` crate, this crate does not contain any direct
//! file access. It is written in such a way that it can be compiled to
//! WebAssembly, with all file access being mediated via a `FileAndPathHelper`
//! trait.
//!
//! Do not use this crate directly unless you have to. Instead, use
//! [`wholesym`](https://docs.rs/wholesym), which provides a much more ergonomic Rust API.
//! `wholesym` exposes the JSON API functionality via [`SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).
//!
//! ## Example
//!
//! ```rust
//! use samply_api::samply_symbols::{
//!     FileContents, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture,
//!     CandidatePathInfo, FileLocation, LibraryInfo, SymbolManager,
//! };
//! use samply_api::samply_symbols::debugid::{CodeId, DebugId};
//!
//! async fn run_query() -> String {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
//!     };
//!     let symbol_manager = SymbolManager::with_helper(helper);
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
//! impl FileAndPathHelper for ExampleHelper {
//!     type F = Vec<u8>;
//!     type FL = ExampleFileLocation;
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
//!         &self,
//!         location: ExampleFileLocation,
//!     ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + '_>> {
//!         Box::pin(async move { Ok(std::fs::read(&location.0)?) })
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
//!
//!     fn location_for_dwo(&self, _comp_dir: &str, path: &str) -> Option<Self> {
//!         Some(Self(std::path::Path::new(path).into()))
//!     }
//!
//!     fn location_for_dwp(&self) -> Option<Self> {
//!         let mut s = self.0.as_os_str().to_os_string();
//!         s.push(".dwp");
//!         Some(Self(s.into()))
//!     }
//! }
//! ```

use asm::AsmApi;
use debugid::DebugId;
pub use samply_symbols;
pub use samply_symbols::debugid;
use samply_symbols::{FileAndPathHelper, SymbolManager};
use serde_json::json;
use source::SourceApi;
use symbolicate::SymbolicateApi;

mod api_file_path;
mod asm;
mod error;
mod hex;
mod source;
mod symbolicate;

pub(crate) fn to_debug_id(breakpad_id: &str) -> Result<DebugId, samply_symbols::Error> {
    // Only accept breakpad IDs with the right syntax, and which aren't all-zeros.
    match DebugId::from_breakpad(breakpad_id) {
        Ok(debug_id) if !debug_id.is_nil() => Ok(debug_id),
        _ => Err(samply_symbols::Error::InvalidBreakpadId(
            breakpad_id.to_string(),
        )),
    }
}

#[derive(Clone, Copy)]
pub struct Api<'a, H: FileAndPathHelper> {
    symbol_manager: &'a SymbolManager<H>,
}

impl<'a, H: FileAndPathHelper> Api<'a, H> {
    /// Create a [`Api`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<H>) -> Self {
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
    ///  - `/asm/v1`: Experimental API. Symbolicates an address and lets you read one of the files in the
    ///    symbol information for that address.
    pub async fn query_api(self, request_url: &str, request_json_data: &str) -> String {
        if request_url == "/symbolicate/v5" {
            let symbolicate_api = SymbolicateApi::new(self.symbol_manager);
            symbolicate_api.query_api_json(request_json_data).await
        } else if request_url == "/source/v1" {
            let source_api = SourceApi::new(self.symbol_manager);
            source_api.query_api_json(request_json_data).await
        } else if request_url == "/asm/v1" {
            let asm_api = AsmApi::new(self.symbol_manager);
            asm_api.query_api_json(request_json_data).await
        } else {
            json!({ "error": format!("Unrecognized URL {request_url}") }).to_string()
        }
    }
}
