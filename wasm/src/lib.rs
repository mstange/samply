mod error;

use js_sys::Promise;
use std::path::Path;
use std::pin::Pin;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

use profiler_get_symbols::{FileByteSource, FileContentsWithChunkedCaching, OptionallySendFuture};

pub use error::{GenericError, GetSymbolsError, JsValueError};

#[wasm_bindgen]
extern "C" {
    pub type FileAndPathHelper;

    /// Returns Array<String>
    /// The strings in the array can be either
    ///   - The path to a binary, or
    ///   - a special string with the syntax "dyldcache:<dyld_cache_path>:<dylib_path>"
    ///     for libraries that are in the dyld shared cache.
    #[wasm_bindgen(catch, method)]
    fn getCandidatePathsForBinaryOrPdb(
        this: &FileAndPathHelper,
        debugName: &str,
        breakpadId: &str,
    ) -> Result<JsValue, JsValue>;

    /// Returns Array<String>
    #[wasm_bindgen(catch, method)]
    fn getCandidatePathsForPdb(
        this: &FileAndPathHelper,
        debugName: &str,
        breakpadId: &str,
        pdbPathAsStoredInBinary: &str,
        binaryPath: &str,
    ) -> Result<JsValue, JsValue>;

    /// Returns Promise<BufferWrapper>
    #[wasm_bindgen(method)]
    fn readFile(this: &FileAndPathHelper, path: &str) -> Promise;

    pub type FileContents;

    #[wasm_bindgen(catch, method)]
    fn getLength(this: &FileContents) -> Result<f64, JsValue>;

    #[wasm_bindgen(catch, method)]
    fn readBytesInto(
        this: &FileContents,
        buffer: js_sys::Uint8Array,
        offset: f64,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method)]
    fn drop(this: &FileContents) -> Result<(), JsValue>;
}

/// Usage:
///
/// ```js
/// async function getSymbolTable(debugName, breakpadId, libKeyToPathMap) {
///   const helper = {
///     getCandidatePathsForBinaryOrPdb: (debugName, breakpadId) => {
///       const path = libKeyToPathMap.get(`${debugName}/${breakpadId}`);
///       if (path !== undefined) {
///         return [path];
///       }
///       return [];
///     },
///     readFile: async (filename) => {
///       const byteLength = await getFileSizeInBytes(filename);
///       const fileHandle = getFileHandle(filename);
///       return {
///         getLength: () => byteLength,
///         readBytesInto: (array, offset) => {
///           syncReadFilePartIntoBuffer(fileHandle, array, offset);
///         },
///         drop: () => {},
///       };
///     },
///   };
///
///   const [addr, index, buffer] = await getCompactSymbolTable(debugName, breakpadId, helper);
///   return [addr, index, buffer];
/// }
/// ```
#[wasm_bindgen(js_name = getCompactSymbolTable)]
pub fn get_compact_symbol_table(
    debug_name: String,
    breakpad_id: String,
    helper: FileAndPathHelper,
) -> Promise {
    // console_error_panic_hook::set_once();
    future_to_promise(get_compact_symbol_table_impl(
        debug_name,
        breakpad_id,
        helper,
    ))
}

/// Usage:
///
/// ```js
/// async function getSymbolTable(url, requestJSONString, libKeyToPathMap) {
///   const helper = {
///     getCandidatePathsForBinaryOrPdb: (debugName, breakpadId) => {
///       const path = libKeyToPathMap.get(`${debugName}/${breakpadId}`);
///       if (path !== undefined) {
///         return [path];
///       }
///       return [];
///     },
///     readFile: async (filename) => {
///       const byteLength = await getFileSizeInBytes(filename);
///       const fileHandle = getFileHandle(filename);
///       return {
///         getLength: () => byteLength,
///         readBytesInto: (array, offset) => {
///           syncReadFilePartIntoBuffer(fileHandle, array, offset);
///         },
///         drop: () => {},
///       };
///     },
///   };
///
///   const responseJSONString = await queryAPI(deburlugName, requestJSONString, helper);
///   return responseJSONString;
/// }
/// ```
#[wasm_bindgen(js_name = queryAPI)]
pub fn query_api(url: String, request_json: String, helper: FileAndPathHelper) -> Promise {
    // console_error_panic_hook::set_once();
    future_to_promise(query_api_impl(url, request_json, helper))
}

async fn query_api_impl(
    url: String,
    request_json: String,
    helper: FileAndPathHelper,
) -> Result<JsValue, JsValue> {
    let response_json = profiler_get_symbols::query_api(&url, &request_json, &helper).await;
    Ok(response_json.into())
}

async fn get_compact_symbol_table_impl(
    debug_name: String,
    breakpad_id: String,
    helper: FileAndPathHelper,
) -> Result<JsValue, JsValue> {
    let result =
        profiler_get_symbols::get_compact_symbol_table(&debug_name, &breakpad_id, &helper).await;
    match result {
        Result::Ok(table) => Ok(js_sys::Array::of3(
            &js_sys::Uint32Array::from(&table.addr[..]),
            &js_sys::Uint32Array::from(&table.index[..]),
            &js_sys::Uint8Array::from(&table.buffer[..]),
        )
        .into()),
        Result::Err(err) => Err(GetSymbolsError::from(err).into()),
    }
}

impl FileContents {
    /// Reads `len` bytes at the offset into the memory at dest_ptr.
    /// Safety: The dest_ptr must point to at least `len` bytes of valid memory, and
    /// exclusive access is granted to this function. The memory may be uninitialized.
    /// Safety: This function guarantees that the `len` bytes at `dest_ptr` will be
    /// fully initialized after the call.
    /// Safety: dest_ptr is not stored and the memory is not accessed after this function
    /// returns.
    /// This function does not accept a rust slice because you have to guarantee that
    /// slice contents are fully initialized before you create a slice, and we want to
    /// allow calling this function with uninitialized memory. It is the point of this
    /// function to do the initialization.
    unsafe fn read_bytes_into(
        &self,
        offset: u64,
        len: usize,
        dest_ptr: *mut u8,
    ) -> Result<(), JsValueError> {
        let array = js_sys::Uint8Array::view_mut_raw(dest_ptr, len);
        // Safety requirements:
        // - readBytesInto must initialize all values in the buffer.
        // - readBytesInto must not call into wasm code which might cause the heap to grow,
        //   because that would invalidate the TypedArray's internal buffer
        // - readBytesInto must not hold on to the array after it has returned
        self.readBytesInto(array, offset as f64)
            .map_err(JsValueError::from)
    }
}

pub struct FileContentsWrapper(FileContents);

impl FileByteSource for FileContentsWrapper {
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: u64,
    ) -> profiler_get_symbols::FileAndPathHelperResult<()> {
        // Make a buffer, wrap a Uint8Array around its bits, and call into JS to fill it.
        // This is implemented in such a way that it avoids zero-initialization and extra
        // copies of the contents.
        let read_len = size as usize;
        buffer.reserve_exact(read_len);
        unsafe {
            // Safety: The buffer has `read_len` bytes of capacity.
            // Safety: Nothing else has a reference to the buffer at the moment; we have exclusive access of its contents.
            self.0
                .read_bytes_into(offset, read_len, buffer.as_mut_ptr().add(buffer.len()))?;
            // Safety: All values in the buffer are now initialized.
            buffer.set_len(buffer.len() + read_len);
        }
        Ok(())
    }
}

impl Drop for FileContentsWrapper {
    fn drop(&mut self) {
        let _ = self.0.drop();
    }
}

impl profiler_get_symbols::FileAndPathHelper for FileAndPathHelper {
    type F = FileContentsWithChunkedCaching<FileContentsWrapper>;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> profiler_get_symbols::FileAndPathHelperResult<Vec<profiler_get_symbols::CandidatePathInfo>>
    {
        get_candidate_paths_for_binary_or_pdb_impl(
            FileAndPathHelper::from((*self).clone()),
            debug_name.to_owned(),
            breakpad_id.to_owned(),
        )
    }

    fn open_file(
        &self,
        path: &Path,
    ) -> Pin<
        Box<
            dyn OptionallySendFuture<
                Output = profiler_get_symbols::FileAndPathHelperResult<Self::F>,
            >,
        >,
    > {
        let helper = FileAndPathHelper::from((*self).clone());
        let path = path.to_owned();
        let future = async move {
            let path = path.to_str().ok_or(GenericError(
                "read_file: Path could not be converted to string",
            ))?;
            let file_res = JsFuture::from(helper.readFile(path)).await;
            let file = file_res.map_err(JsValueError::from)?;
            let contents = FileContents::from(file);
            let len = contents.getLength().map_err(JsValueError::from)? as u64;
            let file_contents_wrapper = FileContentsWrapper(contents);
            Ok(FileContentsWithChunkedCaching::new(
                len,
                file_contents_wrapper,
            ))
        };
        Box::pin(future)
    }
}

fn get_candidate_paths_for_binary_or_pdb_impl(
    helper: FileAndPathHelper,
    debug_name: String,
    breakpad_id: String,
) -> profiler_get_symbols::FileAndPathHelperResult<Vec<profiler_get_symbols::CandidatePathInfo>> {
    let res = helper.getCandidatePathsForBinaryOrPdb(&debug_name, &breakpad_id);
    let value = res.map_err(JsValueError::from)?;
    let array = js_sys::Array::from(&value);
    Ok(array
        .iter()
        .filter_map(|val| val.as_string())
        .map(|s| {
            // Support special syntax "dyldcache:<dyld_cache_path>:<dylib_path>"
            if let Some(remainder) = s.strip_prefix("dyldcache:") {
                if let Some(offset) = remainder.find(':') {
                    let dyld_cache_path = &remainder[0..offset];
                    let dylib_path = &remainder[offset + 1..];
                    return profiler_get_symbols::CandidatePathInfo::InDyldCache {
                        dyld_cache_path: dyld_cache_path.into(),
                        dylib_path: dylib_path.into(),
                    };
                }
            }
            profiler_get_symbols::CandidatePathInfo::Normal(s.into())
        })
        .collect())
}
