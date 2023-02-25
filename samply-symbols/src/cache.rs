use std::{
    collections::HashMap,
    ops::Range,
    sync::{atomic::AtomicUsize, Mutex},
};

use crate::chunked_read_buffer_manager::{ChunkedReadBufferManager, RangeLocation, RangeSourcing};

use elsa::sync::FrozenVec;

use crate::{FileAndPathHelperResult, FileContents};

const CHUNK_SIZE: u64 = 32 * 1024;

#[cfg(not(feature = "send_futures"))]
pub trait FileByteSource {
    /// Read `size` bytes at offset `offset` and append them to `buffer`.
    /// If successful, `buffer` must have had its len increased exactly by `size`,
    /// otherwise the caller may panic.
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()>;
}

#[cfg(feature = "send_futures")]
pub trait FileByteSource: Send + Sync {
    /// Read `size` bytes at offset `offset` and append them to `buffer`.
    /// If successful, `buffer` must have had its len increased exactly by `size`,
    /// otherwise the caller may panic.
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()>;
}

pub struct FileContentsWithChunkedCaching<S: FileByteSource> {
    source: S,
    file_len: u64,
    buffer_manager: Mutex<ChunkedReadBufferManager<CHUNK_SIZE>>,
    string_cache: Mutex<HashMap<(u64, u8), RangeLocation>>,
    buffers: FrozenVec<Box<[u8]>>,
    buffer_count: AtomicUsize,
}

impl<S: FileByteSource> FileContentsWithChunkedCaching<S> {
    pub fn new(file_len: u64, source: S) -> Self {
        FileContentsWithChunkedCaching {
            source,
            buffers: FrozenVec::new(),
            file_len,
            buffer_manager: Mutex::new(ChunkedReadBufferManager::new_with_size(file_len)),
            string_cache: Mutex::new(HashMap::new()),
            buffer_count: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn slice_from_location(&self, location: &RangeLocation) -> &[u8] {
        let buffer = &self.buffers.get(location.buffer_handle).unwrap();
        &buffer[location.offset_from_start..][..location.size]
    }

    /// Must be called with a valid, non-empty range which does not exceed file_len.
    #[inline]
    fn get_range_location(&self, range: Range<u64>) -> FileAndPathHelperResult<RangeLocation> {
        let mut buffer_manager = self.buffer_manager.lock().unwrap();
        let read_range = match buffer_manager.determine_range_sourcing(range.clone()) {
            RangeSourcing::InExistingBuffer(l) => return Ok(l),
            RangeSourcing::NeedToReadNewBuffer(read_range) => read_range,
        };
        assert!(read_range.start <= read_range.end);

        // Read the bytes from the source.
        let read_len: usize = (read_range.end - read_range.start).try_into()?;
        let mut buffer = Vec::new();
        self.source
            .read_bytes_into(&mut buffer, read_range.start, read_len)?;
        assert!(buffer.len() == read_len);

        let buffer_handle = self
            .buffer_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.buffers.push(buffer.into_boxed_slice());
        buffer_manager.insert_buffer_range(read_range.clone(), buffer_handle);

        Ok(RangeLocation {
            buffer_handle,
            offset_from_start: (range.start - read_range.start) as usize,
            size: (range.end - range.start) as usize,
        })
    }
}

impl<S: FileByteSource> FileContents for FileContentsWithChunkedCaching<S> {
    #[inline]
    fn len(&self) -> u64 {
        self.file_len
    }

    #[inline]
    fn read_bytes_at(&self, offset: u64, size: u64) -> FileAndPathHelperResult<&[u8]> {
        if size == 0 {
            return Ok(&[]);
        }

        let start = offset;
        let end = offset.checked_add(size).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read_bytes_at with offset + size overflowing u64",
            )
        })?;
        if end > self.file_len {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read_bytes_at range out-of-bounds",
            )));
        }
        let location = self.get_range_location(start..end)?;
        Ok(self.slice_from_location(&location))
    }

    #[inline]
    fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]> {
        const MAX_LENGTH_INCLUDING_DELIMITER: u64 = 4096;

        if range.end < range.start {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read_bytes_at_until called with range.end < range.start",
            )));
        }
        if range.end > self.file_len {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read_bytes_at_until range out-of-bounds",
            )));
        }

        let mut string_cache = self.string_cache.lock().unwrap();
        if let Some(location) = string_cache.get(&(range.start, delimiter)) {
            return Ok(self.slice_from_location(location));
        }

        let max_len = (range.end - range.start).min(MAX_LENGTH_INCLUDING_DELIMITER);
        let mut location = self.get_range_location(range.start..(range.start + max_len))?;
        let bytes = self.slice_from_location(&location);

        let string_len = match memchr::memchr(delimiter, bytes) {
            Some(len) => len,
            None => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Could not find delimiter",
                )));
            }
        };

        location.size = string_len;
        string_cache.insert((range.start, delimiter), location);
        Ok(&bytes[..string_len])
    }

    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()> {
        self.source.read_bytes_into(buffer, offset, size)
    }
}
