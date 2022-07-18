//! This crate allows obtaining symbol information from binaries and compilation artifacts.
//! It maps raw code addresses to symbol strings, and, if available, file name + line number
//! information.
//! The API was designed for the Firefox profiler.
//!
//! The main entry point of this crate is the async `query_api` function, which accepts a
//! JSON string with the query input. The JSON API matches the API of the [Mozilla
//! symbolication server ("Tecken")](https://tecken.readthedocs.io/en/latest/symbolication.html).
//! An alternative JSON-free API is available too, but it is not very ergonomic.
//!
//! # Design constraints
//!
//! This crate operates under the following design constraints:
//!
//!  - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
//!    WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
//!    This setup allows us to download the profiler-get-symbols wasm bundle on demand, rather than shipping
//!    it with Firefox, which would increase the Firefox download size for a piece of functionality
//!    that the vast majority of Firefox users don't need.
//!  - Performance: We want to be able to obtain symbol data from a fresh build of a locally compiled
//!    Firefox instance as quickly as possible, without an expensive preprocessing step. The time between
//!    "finished compilation" and "returned symbol data" should be minimized. This means that symbol
//!    data needs to be obtained directly from the compilation artifacts rather than from, say, a
//!    dSYM bundle or a Breakpad .sym file.
//!  - Must scale to large inputs: This applies to both the size of the API request and the size of the
//!    object files that need to be parsed: The Firefox profiler will supply anywhere between tens of
//!    thousands and hundreds of thousands of different code addresses in a single symbolication request.
//!    Firefox build artifacts such as libxul.so can be multiple gigabytes big, and contain around 300000
//!    function symbols. We want to serve such requests within a few seconds or less.
//!  - "Best effort" basis: If only limited symbol information is available, for example from system
//!    libraries, we want to return whatever limited information we have.
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
//! ```
//! use samply_api::samply_symbols::{
//!     FileContents, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture,
//!     CandidatePathInfo, FileLocation
//! };
//! use samply_api::samply_symbols::debugid::DebugId;
//!
//! async fn run_query() -> String {
//!     let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//!     let helper = ExampleHelper {
//!         artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
//!     };
//!     samply_api::query_api(
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
//!         &helper,
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
//!     fn get_candidate_paths_for_binary_or_pdb(
//!         &self,
//!         debug_name: &str,
//!         _debug_id: &DebugId,
//!     ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
//!         Ok(vec![CandidatePathInfo::SingleFile(FileLocation::Path(self.artifact_directory.join(debug_name)))])
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

use debugid::DebugId;
use serde_json::json;

mod error;
mod source;
mod symbolicate;

pub(crate) fn to_debug_id(breakpad_id: &str) -> Result<DebugId, samply_symbols::Error> {
    DebugId::from_breakpad(breakpad_id)
        .map_err(|_| samply_symbols::Error::InvalidBreakpadId(breakpad_id.to_string()))
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
///  - `/symbolicate/v5-legacy`: Like v5, but lacking any data that comes from debug information,
///    i.e. files, lines and inlines. This is faster.
///  - `/source/v1`: Experimental API. Symbolicates an address and lets you read one of the files in the
///    symbol information for that address.
pub async fn query_api<'h>(
    request_url: &str,
    request_json_data: &str,
    helper: &'h impl samply_symbols::FileAndPathHelper<'h>,
) -> String {
    if request_url == "/symbolicate/v5-legacy" {
        symbolicate::query_api_json(request_json_data, helper, false).await
    } else if request_url == "/symbolicate/v5" {
        symbolicate::query_api_json(request_json_data, helper, true).await
    } else if request_url == "/source/v1" {
        source::query_api_json(request_json_data, helper).await
    } else {
        json!({ "error": format!("Unrecognized URL {}", request_url) }).to_string()
    }
}
