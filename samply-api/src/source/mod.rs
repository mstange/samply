use crate::to_debug_id;
use samply_symbols::{
    FileAndPathHelper, FileAndPathHelperError, FileContents, FileLocation, FramesLookupResult,
    SymbolManager,
};
use serde_json::json;

mod request_json;
mod response_json;

#[derive(thiserror::Error, Debug)]
enum SourceError {
    #[error("Couldn't parse request: {0}")]
    ParseRequestErrorSerde(#[from] serde_json::error::Error),

    #[error("Could not obtain symbols for the requested library: {0}")]
    NoSymbols(#[from] samply_symbols::Error),

    #[error("Don't have any debug info for the requested library")]
    NoDebugInfo,

    #[error("The requested path is not present in the symbolication frames")]
    InvalidPath,

    #[error("The symbol file came from a non-local origin, so we cannot treat file paths in it as local.")]
    NonLocalSymbols,

    #[error("An error occurred when reading the file: {0}")]
    FileAndPathHelperError(#[from] FileAndPathHelperError),
}

pub struct SourceApi<'a, 'h: 'a, H: FileAndPathHelper<'h>> {
    symbol_manager: &'a SymbolManager<'h, H>,
}

impl<'a, 'h: 'a, H: FileAndPathHelper<'h>> SourceApi<'a, 'h, H> {
    /// Create a [`SourceApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<'h, H>) -> Self {
        Self { symbol_manager }
    }

    pub async fn query_api_json(&self, request_json: &str) -> String {
        match self.query_api_fallible_json(request_json).await {
            Ok(response_json) => response_json,
            Err(err) => json!({ "error": err.to_string() }).to_string(),
        }
    }

    async fn query_api_fallible_json(&self, request_json: &str) -> Result<String, SourceError> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        let response = self.query_api(&request).await?;
        Ok(serde_json::to_string(&response)?)
    }

    async fn query_api(
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
        let frames = {
            let symbol_map = self
                .symbol_manager
                .load_symbol_map(debug_name, debug_id)
                .await?;
            match symbol_map.lookup(*module_offset) {
                Some(address_info) => address_info.frames,
                None => FramesLookupResult::Unavailable,
            }
        };
        let frames = match frames {
            FramesLookupResult::Available(frames) => frames,
            FramesLookupResult::External(external_file_ref, external_file_address) => {
                match self
                    .symbol_manager
                    .lookup_external(&external_file_ref, &external_file_address)
                    .await
                {
                    Some(frames) => frames,
                    None => return Err(SourceError::NoDebugInfo),
                }
            }
            FramesLookupResult::Unavailable => return Err(SourceError::NoDebugInfo),
        };

        // Find the FilePath whose mapped path matches the requested file. This gives us the raw path.
        // This is where we check that the requested file path is permissible.
        let file_path = frames
            .into_iter()
            .filter_map(|frame| frame.file_path)
            .find(|file_path| *file_path.mapped_path() == *requested_file)
            .ok_or(SourceError::InvalidPath)?;

        // One last verification step: Make sure that there's actually a local path for this
        // source file. We will only have a local path if the path was referred to by a local
        // symbol file.
        let local_path = file_path
            .into_local_path()
            .ok_or(SourceError::NonLocalSymbols)?;

        // If we got here, it means that the file access is allowed. Read the file.
        let helper = self.symbol_manager.helper();
        let file_contents = helper.open_file(&FileLocation::Path(local_path)).await?;
        let file_contents = file_contents.read_bytes_at(0, file_contents.len())?;
        let source = String::from_utf8_lossy(file_contents).to_string();

        Ok(response_json::Response {
            symbols_last_modified: None,
            source_last_modified: None,
            file: requested_file.to_string(),
            source,
        })
    }
}
