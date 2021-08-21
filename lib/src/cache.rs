use std::{cell::RefCell, collections::HashMap, ops::Range};

use crate::chunked_read_buffer_manager::{ChunkedReadBufferManager, RangeLocation, RangeSourcing};

use elsa::FrozenVec;

use crate::{FileAndPathHelperResult, FileContents};

const CHUNK_SIZE: u64 = 32 * 1024;

pub trait FileByteSource {
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: u64,
    ) -> FileAndPathHelperResult<()>;
}

pub struct FileContentsWithChunkedCaching<S: FileByteSource> {
    source: S,
    file_len: u64,
    buffer_manager: RefCell<ChunkedReadBufferManager<CHUNK_SIZE>>,
    string_cache: RefCell<HashMap<(u64, u8), RangeLocation>>,
    buffers: FrozenVec<Box<[u8]>>,
}

impl<S: FileByteSource> FileContentsWithChunkedCaching<S> {
    pub fn new(file_len: u64, source: S) -> Self {
        FileContentsWithChunkedCaching {
            source,
            buffers: FrozenVec::new(),
            file_len,
            buffer_manager: RefCell::new(ChunkedReadBufferManager::new_with_size(file_len)),
            string_cache: RefCell::new(HashMap::new()),
        }
    }

    #[inline]
    fn slice_from_location(&self, location: &RangeLocation) -> &[u8] {
        let buffer = &self.buffers[location.buffer_handle];
        &buffer[location.offset_from_start..][..location.size]
    }

    #[inline]
    fn get_range_location(&self, range: Range<u64>) -> FileAndPathHelperResult<RangeLocation> {
        let mut buffer_manager = self.buffer_manager.borrow_mut();
        let read_range = match buffer_manager.determine_range_sourcing(range.clone()) {
            RangeSourcing::InExistingBuffer(l) => return Ok(l),
            RangeSourcing::NeedToReadNewBuffer(read_range) => read_range,
        };

        // Read the bytes from the source.
        let mut buffer = Vec::new();
        self.source.read_bytes_into(
            &mut buffer,
            read_range.start,
            read_range.end - read_range.start,
        )?;

        let buffer_handle = self.buffers.len();
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

        let location = self.get_range_location(offset..(offset + size))?;
        Ok(self.slice_from_location(&location))
    }

    #[inline]
    fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]> {
        const MAX_LENGTH_INCLUDING_DELIMITER: u64 = 4096;

        let mut string_cache = self.string_cache.borrow_mut();
        if let Some(location) = string_cache.get(&(range.start, delimiter)) {
            return Ok(self.slice_from_location(location));
        }

        let max_len = (range.end - range.start).min(MAX_LENGTH_INCLUDING_DELIMITER);
        let mut location = self.get_range_location(range.start..(range.start + max_len))?;
        let bytes = self.slice_from_location(&location);

        let string_len = match bytes.iter().position(|b| *b == delimiter) {
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
        size: u64,
    ) -> FileAndPathHelperResult<()> {
        self.source.read_bytes_into(buffer, offset, size)
    }
}
