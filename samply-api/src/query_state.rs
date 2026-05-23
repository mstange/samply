//! Sans-IO state-machine trait used by the JSON API queries.
//!
//! Each of `/symbolicate/v5`, `/source/v1`, and `/asm/v1` is implemented as a
//! state machine that exposes its file-fetch needs as values. A driver
//! satisfies those needs and feeds the results back in.

use samply_symbols::{BinaryImage, FileLoadError, FileTypes, LibraryInfo, SymbolMap};

use crate::QueryApiJsonResult;

/// What an [`ApiQueryState`] asks the driver for next.
pub enum ApiStep<FT: FileTypes> {
    NeedSymbolMap(LibraryInfo),
    NeedSourceFile {
        debug_file: FT::FL,
        source_file_path: String,
    },
    NeedBinary(LibraryInfo),
    /// Generic raw-file fetch (used for chasing external object / dwo files
    /// during a frames lookup). The state machine handles the result via
    /// [`ApiQueryState::provide_file`].
    NeedFile {
        location: FT::FL,
        required: bool,
    },
    Done,
}

/// A sans-IO state machine for one JSON API query.
///
/// All three queries (`/symbolicate/v5`, `/source/v1`, `/asm/v1`) implement
/// this trait so that a single driver can drive any of them. The driver
/// learns what to fetch via [`Self::poll`], feeds results back via the
/// `provide_*` methods, and consumes the boxed state machine at the end via
/// [`Self::finish`].
pub trait ApiQueryState<FT: FileTypes> {
    fn poll(&self) -> ApiStep<FT>;
    fn provide_symbol_map(&mut self, result: Result<SymbolMap<FT>, samply_symbols::Error>);
    fn provide_source_file(&mut self, result: Result<String, samply_symbols::Error>);
    fn provide_binary(&mut self, result: Result<BinaryImage<FT::F>, samply_symbols::Error>);
    fn provide_file(&mut self, result: Result<FT::F, FileLoadError>);
    fn finish(self: Box<Self>) -> QueryApiJsonResult<FT>;
}
