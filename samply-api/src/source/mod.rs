use samply_symbols::{
    FileAndPathHelper, FileAndPathHelperError, LibraryInfo, LookupAddress, SymbolManager,
};

use crate::api_file_path::to_api_file_path;
use crate::{to_debug_id, Error};

pub mod request_json;
pub mod response_json;

#[derive(thiserror::Error, Debug)]
pub enum SourceError {
    #[error("Could not obtain symbols for the requested library: {0}")]
    NoSymbols(#[from] samply_symbols::Error),

    #[error("Don't have any debug info for the requested library")]
    NoDebugInfo,

    #[error("The requested path is not present in the symbolication frames")]
    InvalidPath,

    #[error("An error occurred when reading the file: {0}")]
    FileAndPathHelperError(#[from] FileAndPathHelperError),
}

pub struct SourceApi<'a, H: FileAndPathHelper> {
    symbol_manager: &'a SymbolManager<H>,
}

impl<'a, H: FileAndPathHelper> SourceApi<'a, H> {
    /// Create a [`SourceApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<H>) -> Self {
        Self { symbol_manager }
    }

    pub async fn query_api_json(
        &self,
        request_json: &str,
    ) -> Result<response_json::Response, Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        Ok(self.query_api(&request).await?)
    }

    pub async fn query_api(
        &self,
        request: &request_json::Request,
    ) -> Result<response_json::Response, SourceError> {
        let request_json::Request {
            debug_id,
            debug_name,
            module_offset,
            file: requested_file,
        } = &request;
        let debug_id = to_debug_id(debug_id)?;

        // Look up the address to see which file paths we are allowed to read.
        let info = LibraryInfo {
            debug_name: Some(debug_name.to_string()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        let symbol_map = self.symbol_manager.load_symbol_map(&info).await?;
        let debug_file_location = symbol_map.debug_file_location().clone();
        let address_info = symbol_map
            .lookup(LookupAddress::Relative(*module_offset))
            .await;
        let frames = address_info
            .and_then(|ai| ai.frames)
            .ok_or(SourceError::NoDebugInfo)?;

        // Find the SourceFilePath whose "api file path" matches the requested file.
        // This is where we check that the requested file path is permissible.
        let source_file_path = frames
            .into_iter()
            .filter_map(|frame| frame.file_path)
            .map(|path| symbol_map.resolve_source_file_path(path))
            .find(|file_path| to_api_file_path(file_path) == *requested_file)
            .ok_or(SourceError::InvalidPath)?;

        // If we got here, it means that the file access is allowed. Read the file.
        let source = self
            .symbol_manager
            .load_source_file(&debug_file_location, &source_file_path)
            .await?;

        Ok(response_json::Response {
            symbols_last_modified: None,
            source_last_modified: None,
            file: requested_file.to_string(),
            source,
        })
    }
}
