mod error;

use js_sys::Promise;
use rangemap::RangeMap;
use std::pin::Pin;
use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

use profiler_get_symbols::OptionallySendFuture;

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

    #[wasm_bindgen(method)]
    fn getLength(this: &FileContents) -> f64;

    #[wasm_bindgen(method)]
    fn readBytesInto(this: &FileContents, offset: f64, size: f64, buffer: js_sys::Uint8Array);

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
///         readBytesInto: (offset, size, array) => {
///           syncReadFilePartIntoBuffer(fileHandle, offset, size, array);
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
///         readBytesInto: (offset, size, array) => {
///           syncReadFilePartIntoBuffer(fileHandle, offset, size, array);
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

impl FileContents {
    /// Reads `len` bytes at the offset into the memory at dest_ptr.
    /// Safety: The dest_ptr must point to at least `len` bytes of valid memory, and
    /// exclusive access is granted to this function.
    /// Safety: This function guarantees that the `len` bytes at `dest_ptr` will be
    /// fully initialized after the call.
    /// Safety: dest_ptr is not stored and the memory is not accessed after this function
    /// returns.
    unsafe fn read_bytes_into(&self, offset: u64, len: usize, dest_ptr: *mut u8) {
        let array = js_sys::Uint8Array::view_mut_raw(dest_ptr, len);
        // Safety requirements:
        // - readBytesInto must initialize all values in the buffer.
        // - readBytesInto must not call into wasm code which might cause the heap to grow,
        //   because that would invalidate the TypedArray's internal buffer
        // - readBytesInto must not hold on to the array after it has returned
        // todo: catch JS exception from readBytesAt
        self.readBytesInto(offset as f64, len as f64, array);
    }
}

const CHUNK_SIZE: u64 = 32 * 1024;

#[inline]
fn round_down_to_multiple(value: u64, factor: u64) -> u64 {
    value / factor * factor
}

#[inline]
fn round_up_to_multiple(value: u64, factor: u64) -> u64 {
    (value + factor - 1) / factor * factor
}

#[derive(Clone)]
struct CachedString {
    file_bytes_index: usize,
    len: usize,
}

struct Cache {
    file_len: u64,
    file_bytes_ranges: Vec<Range<u64>>,
    ranges: RangeMap<u64, usize>,
    strings: HashMap<(u64, u8), CachedString>,
}

impl Cache {
    /// Returns an index into self.file_bytes.
    fn get_or_create_cached_range_including(
        &mut self,
        file_bytes: &elsa::FrozenVec<Box<[u8]>>,
        contents: &FileContents,
        start: u64,
        end: u64,
    ) -> usize {
        assert!(start < end);
        let start_is_cached = if let Some((range, index)) = self.ranges.get_key_value(&start) {
            if end <= range.end {
                return *index;
            }
            true
        } else {
            false
        };

        // The requested range does not exist in self.file_bytes.
        // Compute a range that we want to cache.
        let start = if start_is_cached {
            start
        } else {
            round_down_to_multiple(start, CHUNK_SIZE)
        };
        let end = round_up_to_multiple(end, CHUNK_SIZE).clamp(0, self.file_len);

        // Read it and put it into the self.
        // Make a buffer, wrap a Uint8Array around its bits, and call into JS to fill it.
        // This is implemented in such a way that it avoids zero-initialization and extra
        // copies of the contents.
        let read_len = (end - start) as usize;
        let mut buffer: Vec<u8> = Vec::with_capacity(read_len);
        unsafe {
            // Safety: The buffer has `read_len` bytes of capacity.
            // Safety: Nothing else has a reference to the buffer at the moment; we have exclusive access of its contents.
            contents.read_bytes_into(start, read_len, buffer.as_mut_ptr());
            // Safety: All values in the buffer are now initialized.
            buffer.set_len(read_len);
        }

        let index = (*file_bytes).len();
        file_bytes.push(buffer.into_boxed_slice());
        self.file_bytes_ranges.push(start..end);
        self.ranges.insert(start..end, index);
        index
    }

    fn get_or_create_cached_string_at(
        &mut self,
        file_bytes: &elsa::FrozenVec<Box<[u8]>>,
        contents: &FileContents,
        start: u64,
        max_len: u64,
        delimiter: u8,
    ) -> Option<CachedString> {
        if let Some(s) = self.strings.get(&(start, delimiter)) {
            return Some(s.clone());
        }

        let index =
            self.get_or_create_cached_range_including(file_bytes, contents, start, start + max_len);
        let file_bytes_range = &self.file_bytes_ranges[index];
        let offset_into_range = (start - file_bytes_range.start) as usize;
        let available_length = (file_bytes_range.end - start) as usize;
        let checked_length = available_length.clamp(0, max_len as usize);
        if let Some(len) = file_bytes[index][offset_into_range..][..checked_length]
            .iter()
            .position(|b| *b == delimiter)
        {
            // Found the string! Cache the info in the strings list.
            let cached_string = CachedString {
                file_bytes_index: index,
                len,
            };
            self.strings
                .insert((start, delimiter), cached_string.clone());
            Some(cached_string)
        } else {
            None
        }
    }
}

pub struct FileHandle {
    contents: FileContents,
    len: u64,
    cache: RefCell<Cache>,
    file_bytes: elsa::FrozenVec<Box<[u8]>>,
}

impl profiler_get_symbols::FileContents for FileHandle {
    #[inline]
    fn len(&self) -> u64 {
        self.len
    }

    #[inline]
    fn read_bytes_at(
        &self,
        offset: u64,
        size: u64,
    ) -> profiler_get_symbols::FileAndPathHelperResult<&[u8]> {
        if size == 0 {
            return Ok(&[]);
        }

        let cache = &mut *self.cache.borrow_mut();
        let file_bytes_index = cache.get_or_create_cached_range_including(
            &self.file_bytes,
            &self.contents,
            offset,
            offset + size,
        );
        let file_bytes = &self.file_bytes[file_bytes_index];
        let file_bytes_range_start = cache.file_bytes_ranges[file_bytes_index].start;
        let offset_into_range = (offset - file_bytes_range_start) as usize;
        let buf = &file_bytes[offset_into_range..][..size as usize];
        Ok(buf)
    }

    #[inline]
    fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> profiler_get_symbols::FileAndPathHelperResult<&[u8]> {
        const MAX_LENGTH_INCLUDING_DELIMITER: u64 = 4096;

        let cache = &mut *self.cache.borrow_mut();

        let max_len = (range.end - range.start).min(MAX_LENGTH_INCLUDING_DELIMITER);
        let s = match cache.get_or_create_cached_string_at(
            &self.file_bytes,
            &self.contents,
            range.start,
            max_len,
            delimiter,
        ) {
            Some(s) => s,
            None => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Could not find delimiter",
                )))
            }
        };
        let file_bytes = &self.file_bytes[s.file_bytes_index];
        let file_bytes_range_start = cache.file_bytes_ranges[s.file_bytes_index].start;
        let offset_into_range = (range.start - file_bytes_range_start) as usize;
        let buf = &file_bytes[offset_into_range..][..s.len];
        Ok(buf)
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

async fn read_file_impl(
    helper: FileAndPathHelper,
    path: PathBuf,
) -> profiler_get_symbols::FileAndPathHelperResult<FileHandle> {
    let path = path.to_str().ok_or(GenericError(
        "read_file: Path could not be converted to string",
    ))?;
    let file_res = JsFuture::from(helper.readFile(path)).await;
    let file = file_res.map_err(JsValueError::from)?;
    let contents = FileContents::from(file);
    let len = contents.getLength() as u64;
    let cache = RefCell::new(Cache {
        file_len: len,
        file_bytes_ranges: Vec::new(),
        ranges: RangeMap::new(),
        strings: HashMap::new(),
    });
    let file_handle = FileHandle {
        contents,
        file_bytes: elsa::FrozenVec::new(),
        len,
        cache,
    };
    Ok(file_handle)
}
