use profiler_get_symbols;
use wasm_bindgen;

mod compact_symbol_table;
mod error;
mod wasm_mem_buffer;

use js_sys::Promise;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

pub use compact_symbol_table::CompactSymbolTable;
pub use error::{GenericError, GetSymbolsError, JsValueError};
pub use wasm_mem_buffer::WasmMemBuffer;

#[wasm_bindgen]
extern "C" {
    pub type FileAndPathHelper;

    /// Returns Promise<Array<String>>
    #[wasm_bindgen(method)]
    fn getCandidatePathsForBinaryOrPdb(
        this: &FileAndPathHelper,
        debugName: &str,
        breakpadId: &str,
    ) -> Promise;

    /// Returns Promise<Array<String>>
    #[wasm_bindgen(method)]
    fn getCandidatePathsForPdb(
        this: &FileAndPathHelper,
        debugName: &str,
        breakpadId: &str,
        pdbPathAsStoredInBinary: &str,
        binaryPath: &str,
    ) -> Promise;

    /// Returns Promise<BufferWrapper>
    #[wasm_bindgen(method)]
    fn readFile(this: &FileAndPathHelper, path: &str) -> Promise;

    pub type BufferWrapper;

    #[wasm_bindgen(method)]
    fn getBuffer(this: &BufferWrapper) -> WasmMemBuffer;
}

#[wasm_bindgen(js_name = getCompactSymbolTable)]
pub fn get_compact_symbol_table(
    debug_name: String,
    breakpad_id: String,
    helper: FileAndPathHelper,
) -> Promise {
    future_to_promise(get_compact_symbol_table_impl(
        debug_name,
        breakpad_id,
        helper,
    ))
}

async fn get_compact_symbol_table_impl(
    debug_name: String,
    breakpad_id: String,
    helper: FileAndPathHelper,
) -> Result<JsValue, JsValue> {
    let result =
        profiler_get_symbols::get_compact_symbol_table_async(&debug_name, &breakpad_id, &helper)
            .await;
    match result {
        Result::Ok(table) => Ok(CompactSymbolTable::from(table).into()),
        Result::Err(err) => Err(GetSymbolsError::from(err).into()),
    }
}

impl profiler_get_symbols::FileAndPathHelper for FileAndPathHelper {
    type FileContents = WasmMemBuffer;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> Pin<Box<dyn Future<Output = profiler_get_symbols::FileAndPathHelperResult<Vec<PathBuf>>>>>
    {
        Box::pin(get_candidate_paths_for_binary_or_pdb_impl(
            FileAndPathHelper::from((*self).clone()),
            debug_name.to_owned(),
            breakpad_id.to_owned(),
        ))
    }

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<
        Box<dyn Future<Output = profiler_get_symbols::FileAndPathHelperResult<Self::FileContents>>>,
    > {
        Box::pin(read_file_impl(
            FileAndPathHelper::from((*self).clone()),
            path.to_owned(),
        ))
    }
}

async fn get_candidate_paths_for_binary_or_pdb_impl(
    helper: FileAndPathHelper,
    debug_name: String,
    breakpad_id: String,
) -> profiler_get_symbols::FileAndPathHelperResult<Vec<PathBuf>> {
    let res =
        JsFuture::from(helper.getCandidatePathsForBinaryOrPdb(&debug_name, &breakpad_id)).await;
    let value = res.map_err(JsValueError::from)?;
    let array = js_sys::Array::from(&value);
    Ok(array
        .iter()
        .filter_map(|val| val.as_string().map(|s| s.into()))
        .collect())
}

async fn read_file_impl(
    helper: FileAndPathHelper,
    path: PathBuf,
) -> profiler_get_symbols::FileAndPathHelperResult<WasmMemBuffer> {
    let path = path.to_str().ok_or(GenericError(
        "read_file: Path could not be converted to string",
    ))?;
    let res = JsFuture::from(helper.readFile(path)).await;
    let buffer = res.map_err(JsValueError::from)?;
    // Workaround for not having WasmMemBuffer::from(JsValue)
    Ok(BufferWrapper::from(buffer).getBuffer())
}
