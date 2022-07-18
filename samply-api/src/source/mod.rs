use std::ops::Deref;

use crate::to_debug_id;
use samply_symbols::{
    AddressDebugInfo, FileAndPathHelper, FileAndPathHelperError, FileContents, FileLocation,
    InlineStackFrame, SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use serde_json::json;

mod request_json;
mod response_json;

pub struct FramesForSingleAddress {
    pub address: u32,
    pub frames: Option<Vec<InlineStackFrame>>,
}

impl SymbolicationResult for FramesForSingleAddress {
    fn from_full_map<T: Deref<Target = str>>(_symbols: Vec<(u32, T)>) -> Self {
        panic!("Should not be called")
    }

    fn for_addresses(addresses: &[u32]) -> Self {
        assert!(
            addresses.len() == 1,
            "Should only be used with a single address"
        );
        FramesForSingleAddress {
            address: addresses[0],
            frames: None,
        }
    }

    fn add_address_symbol(
        &mut self,
        address: u32,
        _symbol_address: u32,
        _symbol_name: String,
        _function_size: Option<u32>,
    ) {
        assert!(address == self.address, "Unexpected address");
    }

    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo) {
        assert!(address == self.address, "Unexpected address");
        self.frames = Some(info.frames);
    }

    fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {}
}

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

pub async fn query_api_json<'h>(
    request_json: &str,
    helper: &'h impl FileAndPathHelper<'h>,
) -> String {
    match query_api_fallible_json(request_json, helper).await {
        Ok(response_json) => response_json,
        Err(err) => json!({ "error": err.to_string() }).to_string(),
    }
}

async fn query_api_fallible_json<'h>(
    request_json: &str,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<String, SourceError> {
    let request: request_json::Request = serde_json::from_str(request_json)?;
    let response = query_api(&request, helper).await?;
    Ok(serde_json::to_string(&response)?)
}

async fn query_api<'h>(
    request: &request_json::Request,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<response_json::Response, SourceError> {
    let request_json::Request {
        debug_id,
        debug_name,
        module_offset,
        file: requested_file,
    } = &request;

    // Look up the address to see which file paths we are allowed to read.
    let symbol_result: FramesForSingleAddress = samply_symbols::get_symbolication_result(
        SymbolicationQuery {
            debug_name,
            debug_id: to_debug_id(debug_id)?,
            result_kind: SymbolicationResultKind::SymbolsForAddresses {
                addresses: &[*module_offset],
                with_debug_info: true,
            },
        },
        helper,
    )
    .await?;

    // Find the FilePath whose mapped path matches the requested file. This gives us the raw path.
    // This is where we check that the requested file path is permissible.
    let file_path = symbol_result
        .frames
        .ok_or(SourceError::NoDebugInfo)?
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
