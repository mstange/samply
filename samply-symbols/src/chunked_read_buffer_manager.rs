use rangemap::RangeMap;
use std::ops::Range;

pub struct ChunkedReadBufferManager<const CHUNK_SIZE: u64> {
    file_len: u64,
    buffer_ranges: Vec<BufferRange>,
    range_map: RangeMap<u64, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeLocation {
    pub buffer_handle: usize,
    pub offset_from_start: usize,
    pub size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeSourcing {
    InExistingBuffer(RangeLocation),
    NeedToReadNewBuffer(Range<u64>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BufferRange {
    pub range: Range<u64>,
    pub buffer_handle: usize,
}

#[inline]
fn round_down_to_multiple(value: u64, factor: u64) -> u64 {
    value / factor * factor
}

#[inline]
fn round_up_to_multiple(value: u64, factor: u64) -> u64 {
    (value + factor - 1) / factor * factor
}

impl<const CHUNK_SIZE: u64> ChunkedReadBufferManager<CHUNK_SIZE> {
    pub fn new_with_size(file_len: u64) -> Self {
        ChunkedReadBufferManager {
            file_len,
            buffer_ranges: Vec::new(),
            range_map: RangeMap::new(),
        }
    }

    /// Must be called with a valid, non-empty range which does not exceed file_len.
    pub fn determine_range_sourcing(&self, range: Range<u64>) -> RangeSourcing {
        assert!(range.start < range.end);
        assert!(range.end <= self.file_len);

        let start_is_cached = if let Some(buffer_range_index) = self.range_map.get(&range.start) {
            let buffer_range = &self.buffer_ranges[*buffer_range_index];
            if range.end <= buffer_range.range.end {
                return RangeSourcing::InExistingBuffer(RangeLocation {
                    buffer_handle: buffer_range.buffer_handle,
                    offset_from_start: (range.start - buffer_range.range.start) as usize,
                    size: (range.end - range.start) as usize,
                });
            }
            true
        } else {
            false
        };

        // The requested range does not exist in self.buffers.
        // Compute a range that we want to read into a buffer and cache.
        let start = if start_is_cached {
            range.start
        } else {
            round_down_to_multiple(range.start, CHUNK_SIZE)
        };
        let end = round_up_to_multiple(range.end, CHUNK_SIZE).clamp(0, self.file_len);
        RangeSourcing::NeedToReadNewBuffer(start..end)
    }

    /// Panics if range.end < range.start
    pub fn insert_buffer_range(&mut self, range: Range<u64>, buffer_handle: usize) {
        let index = self.buffer_ranges.len();
        self.buffer_ranges.push(BufferRange {
            range: range.clone(),
            buffer_handle,
        });
        self.range_map.insert(range, index);
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkedReadBufferManager, RangeLocation, RangeSourcing};

    #[test]
    fn rounds_out_to_chunks() {
        let manager = ChunkedReadBufferManager::<10>::new_with_size(55);
        assert_eq!(
            manager.determine_range_sourcing(3..5),
            RangeSourcing::NeedToReadNewBuffer(0..10)
        );
        assert_eq!(
            manager.determine_range_sourcing(27..28),
            RangeSourcing::NeedToReadNewBuffer(20..30)
        );
        assert_eq!(
            manager.determine_range_sourcing(27..30),
            RangeSourcing::NeedToReadNewBuffer(20..30)
        );
        assert_eq!(
            manager.determine_range_sourcing(20..28),
            RangeSourcing::NeedToReadNewBuffer(20..30)
        );
        assert_eq!(
            manager.determine_range_sourcing(27..31),
            RangeSourcing::NeedToReadNewBuffer(20..40)
        );
        assert_eq!(
            manager.determine_range_sourcing(19..28),
            RangeSourcing::NeedToReadNewBuffer(10..30)
        );
        assert_eq!(
            manager.determine_range_sourcing(19..30),
            RangeSourcing::NeedToReadNewBuffer(10..30)
        );
        assert_eq!(
            manager.determine_range_sourcing(15..33),
            RangeSourcing::NeedToReadNewBuffer(10..40)
        );
        // Check that ranges at the end don't overflow the file size.
        assert_eq!(
            manager.determine_range_sourcing(48..53),
            RangeSourcing::NeedToReadNewBuffer(40..55)
        );
    }

    #[test]
    fn finds_existing_ranges() {
        let mut manager = ChunkedReadBufferManager::<10>::new_with_size(55);
        assert_eq!(
            manager.determine_range_sourcing(3..5),
            RangeSourcing::NeedToReadNewBuffer(0..10)
        );
        manager.insert_buffer_range(0..10, 5);
        assert_eq!(
            manager.determine_range_sourcing(3..8),
            RangeSourcing::InExistingBuffer(RangeLocation {
                buffer_handle: 5,
                offset_from_start: 3,
                size: 5
            })
        );
        assert_eq!(
            manager.determine_range_sourcing(24..26),
            RangeSourcing::NeedToReadNewBuffer(20..30)
        );
        manager.insert_buffer_range(20..30, 17);
        assert_eq!(
            manager.determine_range_sourcing(23..29),
            RangeSourcing::InExistingBuffer(RangeLocation {
                buffer_handle: 17,
                offset_from_start: 3,
                size: 6
            })
        );
    }

    #[test]
    fn last_buffer_wins() {
        let mut manager = ChunkedReadBufferManager::<10>::new_with_size(55);
        assert_eq!(
            manager.determine_range_sourcing(13..15),
            RangeSourcing::NeedToReadNewBuffer(10..20)
        );
        manager.insert_buffer_range(10..20, 7);
        assert_eq!(
            manager.determine_range_sourcing(10..28),
            RangeSourcing::NeedToReadNewBuffer(10..30)
        );
        manager.insert_buffer_range(10..20, 23);
        assert_eq!(
            manager.determine_range_sourcing(13..15),
            RangeSourcing::InExistingBuffer(RangeLocation {
                buffer_handle: 23,
                offset_from_start: 3,
                size: 2
            })
        );
    }

    #[test]
    fn not_rounding_down_when_start_straddles_into_old_chunk() {
        let mut manager = ChunkedReadBufferManager::<10>::new_with_size(55);
        assert_eq!(
            manager.determine_range_sourcing(13..18),
            RangeSourcing::NeedToReadNewBuffer(10..20)
        );
        manager.insert_buffer_range(10..20, 2);
        assert_eq!(
            manager.determine_range_sourcing(18..23),
            RangeSourcing::NeedToReadNewBuffer(18..30)
        );
        manager.insert_buffer_range(18..30, 25);
        assert_eq!(
            manager.determine_range_sourcing(18..20),
            RangeSourcing::InExistingBuffer(RangeLocation {
                buffer_handle: 25,
                offset_from_start: 0,
                size: 2
            })
        );
        assert_eq!(
            manager.determine_range_sourcing(17..20),
            RangeSourcing::InExistingBuffer(RangeLocation {
                buffer_handle: 2,
                offset_from_start: 7,
                size: 3
            })
        );
    }
}
