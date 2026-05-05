//! This crate implements a JSON API for profiler symbolication with the help of
//! local symbol files. It exposes a single type called `API`, and uses the
//! `samply-symbols` crate for its implementation.
//!
//! Just like the `samply-symbols` crate, this crate does not contain any direct
//! file access. It is written in such a way that it can be compiled to
//! WebAssembly. The state machines exposed here are generic over a [`FileTypes`]
//! type bundle, and a driver in the consumer's environment performs the actual
//! file I/O.
//!
//! Do not use this crate directly unless you have to. Instead, use
//! [`wholesym`](https://docs.rs/wholesym), which provides a much more ergonomic Rust API.
//! `wholesym` exposes the JSON API functionality via [`SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).
//!
//! ## Example
//!
//! `samply-api` itself is sans-IO: [`Api::build_query`] returns a state
//! machine which surfaces "I need this file / symbol map / binary"
//! requests as values via [`ApiQueryState::poll`]. A driver — typically
//! `wholesym` — fetches what's requested and feeds the result back in.
//! For an end-to-end usage example, see
//! [`wholesym::SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).

use debugid::DebugId;
pub use samply_symbols;
pub use samply_symbols::debugid;
use samply_symbols::FileTypes;
use serde::Serialize;

mod api_file_path;
mod asm;
mod error;
mod hex;
mod query_state;
mod source;
mod symbolicate;

pub use asm::AsmApiQueryState;
pub use error::Error;
pub use query_state::{ApiQueryState, ApiStep};
pub use source::SourceApiQueryState;
pub use symbolicate::SymbolicateApiQueryState;

/// The outcome of loading symbols for one module during a `/symbolicate/v5` request.
#[derive(Debug, Clone)]
pub enum ModuleLoadOutcome {
    /// Symbols were loaded successfully.
    Loaded,
    /// Symbol loading failed.
    ///
    /// `error_name` is a stable identifier for the error kind (the enum variant name),
    /// suitable for use as a metric tag or log field.
    Failed { error_name: &'static str },
}

/// Per-module observability data collected during a `/symbolicate/v5` request.
///
/// The `load_duration` covers the full cost of making the module's symbols
/// available: cache lookup, download (if needed), and parsing.  Download timing
/// is also reported independently via [`wholesym::DownloaderObserver`] callbacks,
/// which fire for every download regardless of which request triggered it.
#[derive(Debug, Clone)]
pub struct ModuleStat {
    pub debug_name: String,
    pub breakpad_id: String,
    pub outcome: ModuleLoadOutcome,
}

/// Observability data for a `/symbolicate/v5` request.
///
/// Returned alongside the symbolication result so that callers can emit
/// per-request metrics and structured log entries without having to parse the
/// JSON response.  It is **not** included in the serialized JSON output.
#[derive(Debug, Clone)]
pub struct SymbolicateStats {
    /// Number of jobs in the request.
    pub jobs_count: usize,
    /// Total number of stacks across all jobs.
    pub stacks_count: usize,
    /// Total number of frames across all jobs.
    pub frames_count: usize,
    /// One entry per unique (debug_name, breakpad_id) pair that was looked up.
    pub module_stats: Vec<ModuleStat>,
}

pub(crate) fn to_debug_id(breakpad_id: &str) -> Result<DebugId, samply_symbols::Error> {
    // Only accept breakpad IDs with the right syntax, and which aren't all-zeros.
    match DebugId::from_breakpad(breakpad_id) {
        Ok(debug_id) if !debug_id.is_nil() => Ok(debug_id),
        _ => Err(samply_symbols::Error::InvalidBreakpadId(
            breakpad_id.to_string(),
        )),
    }
}

pub enum QueryApiJsonResult<H: FileTypes> {
    SymbolicateResponse(symbolicate::response_json::Response<H>),
    SourceResponse(source::response_json::Response),
    AsmResponse(asm::response_json::Response),
    Err(Error),
}

impl<H: FileTypes> From<symbolicate::response_json::Response<H>> for QueryApiJsonResult<H> {
    fn from(value: symbolicate::response_json::Response<H>) -> Self {
        QueryApiJsonResult::SymbolicateResponse(value)
    }
}

impl<H: FileTypes> From<source::response_json::Response> for QueryApiJsonResult<H> {
    fn from(value: source::response_json::Response) -> Self {
        QueryApiJsonResult::SourceResponse(value)
    }
}

impl<H: FileTypes> From<asm::response_json::Response> for QueryApiJsonResult<H> {
    fn from(value: asm::response_json::Response) -> Self {
        QueryApiJsonResult::AsmResponse(value)
    }
}

impl<H: FileTypes> From<Error> for QueryApiJsonResult<H> {
    fn from(value: Error) -> Self {
        QueryApiJsonResult::Err(value)
    }
}

impl<H: FileTypes> QueryApiJsonResult<H> {
    /// Returns the HTTP status code that best describes this result.
    pub fn http_status(&self) -> u16 {
        match self {
            QueryApiJsonResult::Err(e) => e.http_status(),
            _ => 200,
        }
    }

    /// Returns observability statistics for `/symbolicate/v5` requests.
    ///
    /// Returns `None` for `/asm/v1`, `/source/v1`, and error responses.
    pub fn symbolicate_stats(&self) -> Option<&SymbolicateStats> {
        match self {
            QueryApiJsonResult::SymbolicateResponse(r) => Some(&r.stats),
            _ => None,
        }
    }
}

impl<H: FileTypes> Serialize for QueryApiJsonResult<H> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            QueryApiJsonResult::SymbolicateResponse(response) => response.serialize(serializer),
            QueryApiJsonResult::SourceResponse(response) => response.serialize(serializer),
            QueryApiJsonResult::AsmResponse(response) => response.serialize(serializer),
            QueryApiJsonResult::Err(error) => error.serialize(serializer),
        }
    }
}

/// The "Tecken" JSON API, exposed as a sans-IO state machine.
///
/// The supported "URLs" are:
///  - `/symbolicate/v5`: This API is documented at <https://tecken.readthedocs.io/en/latest/symbolication.html>.
///    The returned data has two extra fields: inlines (per address) and module_errors (per job).
///  - `/source/v1`: Experimental API. Symbolicates an address and lets you read one of the files in the
///    symbol information for that address.
///  - `/asm/v1`: Experimental API. Symbolicates an address and lets you read one of the files in the
///    symbol information for that address.
pub struct Api;

impl Api {
    /// Build a query state machine for the given URL and JSON request body.
    ///
    /// The returned state machine surfaces "I need this file / symbol map /
    /// binary" requests via [`ApiQueryState::poll`], which a driver
    /// satisfies by calling the corresponding `provide_*` method. When the
    /// state machine reaches [`ApiStep::Done`], pass it to
    /// [`ApiQueryState::finish`] to get the final [`QueryApiJsonResult`].
    pub fn build_query<H: FileTypes + Send + Sync + 'static>(
        request_url: &str,
        request_json_data: &str,
    ) -> Result<Box<dyn ApiQueryState<H> + Send>, Error>
    where
        H::FL: Send + Sync,
    {
        match request_url {
            "/symbolicate/v5" => {
                let state = SymbolicateApiQueryState::<H>::from_request_json(request_json_data)?;
                Ok(Box::new(state))
            }
            "/source/v1" => {
                let state = SourceApiQueryState::<H>::from_request_json(request_json_data)?;
                Ok(Box::new(state))
            }
            "/asm/v1" => {
                let state = AsmApiQueryState::<H>::from_request_json(request_json_data)?;
                Ok(Box::new(state))
            }
            _ => Err(Error::UnrecognizedUrl(request_url.into())),
        }
    }
}
