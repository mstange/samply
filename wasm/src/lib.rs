use profiler_get_symbols;
use wasm_bindgen;

mod error;
mod wasm_mem_buffer;

use js_sys::Promise;
use std::{
    cell::RefCell,
    cmp::min,
    collections::{hash_map::Entry, HashMap},
    path::{Path, PathBuf},
};
use std::{mem, pin::Pin};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

use profiler_get_symbols::OptionallySendFuture;

pub use error::{GenericError, GetSymbolsError, JsValueError};
pub use wasm_mem_buffer::WasmMemBuffer;

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

    #[wasm_bindgen(method)]
    fn getLength(this: &FileContents) -> f64;

    #[wasm_bindgen(method)]
    fn readBytesAt(this: &FileContents, offset: f64, size: f64) -> WasmMemBuffer;

    #[wasm_bindgen(method)]
    fn drop(this: &FileContents);
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
///         readBytesAt: (offset, size) => {
///           return new WasmMemBuffer(size, array => {
///             syncReadFilePartIntoBuffer(fileHandle, offset, size, array);
///           });
///         },
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
///         readBytesAt: (offset, size) => {
///           return new WasmMemBuffer(size, array => {
///             syncReadFilePartIntoBuffer(fileHandle, offset, size, array);
///           });
///         },
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

pub struct FileHandle {
    contents: FileContents,
    cache: RefCell<HashMap<(u64, u64), Box<[u8]>>>,
    string_cache: RefCell<HashMap<(u64, u8), Box<[u8]>>>,
}

impl profiler_get_symbols::FileContents for FileHandle {
    #[inline]
    fn len(&self) -> u64 {
        self.contents.getLength() as u64
    }

    #[inline]
    fn read_bytes_at<'a>(
        &'a self,
        offset: u64,
        size: u64,
    ) -> profiler_get_symbols::FileAndPathHelperResult<&'a [u8]> {
        let cache = &mut *self.cache.borrow_mut();
        let buf = cache.entry((offset, size)).or_insert_with(|| {
            self.contents
                .readBytesAt(offset as f64, size as f64) // todo: catch JS exception from readBytesAt
                .into_buffer()
                .into_boxed_slice()
        });
        let buf = &buf[..];
        // Extend the lifetime to that of self.
        // This is OK because we never mutate or remove entries in self.cache.
        Ok(unsafe { mem::transmute::<&[u8], &[u8]>(buf) })
    }

    #[inline]
    fn read_bytes_at_until<'a>(
        &'a self,
        offset: u64,
        delimiter: u8,
    ) -> profiler_get_symbols::FileAndPathHelperResult<&'a [u8]> {
        let cache = &mut *self.string_cache.borrow_mut();
        let buf = match cache.entry((offset, delimiter)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let mut bytes = Vec::new();
                let mut checked = 0;
                let mut remaining_len: u64 = self.len() - offset;
                loop {
                    let chunk_size = min(256, remaining_len);
                    let chunk: Vec<u8> = self
                        .contents
                        .readBytesAt((offset + checked) as f64, chunk_size as f64) // todo: catch JS exception from readBytesAt
                        .into_buffer();

                    if let Some(pos) = chunk.iter().position(|b| *b == delimiter) {
                        bytes.extend_from_slice(&chunk[..pos]);
                        break entry.insert(bytes.into_boxed_slice());
                    }

                    bytes.extend_from_slice(&chunk);
                    checked += chunk_size;
                    remaining_len -= chunk_size;
                    // Strings should be relatively small.
                    // TODO: make this configurable?
                    if checked > 4096 || remaining_len == 0 {
                        return Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Could not find delimiter",
                        )));
                    }
                }
            }
        };
        // Extend the lifetime to that of self.
        // This is OK because we never mutate or remove entries.
        Ok(unsafe { mem::transmute::<&[u8], &[u8]>(buf) })
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        self.contents.drop();
    }
}

impl profiler_get_symbols::FileAndPathHelper for FileAndPathHelper {
    type F = FileHandle;

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
        Box::pin(read_file_impl(
            FileAndPathHelper::from((*self).clone()),
            path.to_owned(),
        ))
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
                if let Some(offset) = remainder.find(":") {
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

async fn read_file_impl(
    helper: FileAndPathHelper,
    path: PathBuf,
) -> profiler_get_symbols::FileAndPathHelperResult<FileHandle> {
    let path = path.to_str().ok_or(GenericError(
        "read_file: Path could not be converted to string",
    ))?;
    let file_res = JsFuture::from(helper.readFile(path)).await;
    let file = file_res.map_err(JsValueError::from)?;
    let file_handle = FileHandle {
        contents: FileContents::from(file),
        cache: RefCell::new(HashMap::new()),
        string_cache: RefCell::new(HashMap::new()),
    };
    Ok(file_handle)
}
