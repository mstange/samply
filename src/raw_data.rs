use std::borrow::Cow;
use std::cmp::min;
use std::fmt;
use std::mem;
use std::ops::Range;

use crate::utils::HexValue;
use byteorder::{ByteOrder, NativeEndian};

/// A slice of u8 data that can have non-contiguous backing storage split
/// into two pieces, and abstracts that split away so that users can pretend
/// to deal with a contiguous slice.
/// When reading perf events from the mmap'd fd that contains the perf event
/// stream, it often happens that a single event straddles the boundary between
/// two mmap chunks, or is wrapped from the end to the start of a chunk.
#[derive(Clone, Copy)]
pub enum RawData<'a> {
    Single(&'a [u8]),
    #[allow(unused)]
    Split(&'a [u8], &'a [u8]),
}

impl<'a> From<&'a Cow<'a, [u8]>> for RawData<'a> {
    fn from(data: &'a Cow<'a, [u8]>) -> Self {
        match *data {
            Cow::Owned(ref bytes) => RawData::Single(bytes.as_slice()),
            Cow::Borrowed(bytes) => RawData::Single(bytes),
        }
    }
}

impl<'a> From<&'a [u8]> for RawData<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        RawData::Single(bytes)
    }
}

impl<'a> fmt::Debug for RawData<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match *self {
            RawData::Single(buffer) => write!(fmt, "RawData::Single( [u8; {}] )", buffer.len()),
            RawData::Split(left, right) => write!(
                fmt,
                "RawData::Split( [u8; {}], [u8; {}] )",
                left.len(),
                right.len()
            ),
        }
    }
}

impl<'a> RawData<'a> {
    #[allow(unused)]
    #[inline]
    pub(crate) fn empty() -> Self {
        RawData::Single(&[])
    }

    pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), std::io::Error> {
        let buf_len = buf.len();
        *self = match *self {
            RawData::Single(single) => {
                if single.len() < buf_len {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                buf.copy_from_slice(&single[..buf_len]);
                RawData::Single(&single[buf_len..])
            }
            RawData::Split(left, right) => {
                if buf_len <= left.len() {
                    buf.copy_from_slice(&left[..buf_len]);
                    if buf_len < left.len() {
                        RawData::Split(&left[buf_len..], right)
                    } else {
                        RawData::Single(right)
                    }
                } else {
                    let remainder_len = buf_len - left.len();
                    if remainder_len > right.len() {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    buf.copy_from_slice(left);
                    buf.copy_from_slice(&right[..remainder_len]);
                    RawData::Single(&right[remainder_len..])
                }
            }
        };
        Ok(())
    }

    pub fn read_u64<T: ByteOrder>(&mut self) -> Result<u64, std::io::Error> {
        let mut b = [0; 8];
        self.read_exact(&mut b)?;
        Ok(T::read_u64(&b))
    }

    pub fn read_u32<T: ByteOrder>(&mut self) -> Result<u32, std::io::Error> {
        let mut b = [0; 4];
        self.read_exact(&mut b)?;
        Ok(T::read_u32(&b))
    }

    pub fn read_i32<T: ByteOrder>(&mut self) -> Result<i32, std::io::Error> {
        let mut b = [0; 4];
        self.read_exact(&mut b)?;
        Ok(T::read_i32(&b))
    }

    pub fn read_u16<T: ByteOrder>(&mut self) -> Result<u16, std::io::Error> {
        let mut b = [0; 2];
        self.read_exact(&mut b)?;
        Ok(T::read_u16(&b))
    }

    pub fn read_u8(&mut self) -> Result<u8, std::io::Error> {
        let mut b = [0; 1];
        self.read_exact(&mut b)?;
        Ok(b[0])
    }

    /// Finds the first nul byte. Returns everything before that nul byte.
    /// Sets self to everything after the nul byte.
    pub fn read_string(&mut self) -> Option<RawData<'a>> {
        let (rv, new_self) = match *self {
            RawData::Single(single) => {
                let n = memchr::memchr(0, single)?;
                (
                    RawData::Single(&single[..n]),
                    RawData::Single(&single[n + 1..]),
                )
            }
            RawData::Split(left, right) => {
                if let Some(n) = memchr::memchr(0, left) {
                    (
                        RawData::Single(&left[..n]),
                        if n + 1 < left.len() {
                            RawData::Split(&left[n + 1..], right)
                        } else {
                            RawData::Single(right)
                        },
                    )
                } else if let Some(n) = memchr::memchr(0, right) {
                    (
                        RawData::Split(left, &right[..n]),
                        RawData::Single(&right[n + 1..]),
                    )
                } else {
                    return None;
                }
            }
        };
        *self = new_self;
        Some(rv)
    }

    /// Returns the first `n` bytes, and sets self to the remainder.
    pub fn split_off_prefix(&mut self, n: usize) -> Result<Self, std::io::Error> {
        let (rv, new_self) = match *self {
            RawData::Single(single) => {
                if single.len() < n {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                (RawData::Single(&single[..n]), RawData::Single(&single[n..]))
            }
            RawData::Split(left, right) => {
                if n <= left.len() {
                    (
                        RawData::Single(&left[..n]),
                        if n < left.len() {
                            RawData::Split(&left[n..], right)
                        } else {
                            RawData::Single(right)
                        },
                    )
                } else {
                    let remainder_len = n - left.len();
                    if remainder_len > right.len() {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    (
                        RawData::Split(left, &right[..remainder_len]),
                        RawData::Single(&right[remainder_len..]),
                    )
                }
            }
        };
        *self = new_self;
        Ok(rv)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), std::io::Error> {
        *self = match *self {
            RawData::Single(single) => {
                if single.len() < n {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                RawData::Single(&single[n..])
            }
            RawData::Split(left, right) => {
                if n < left.len() {
                    RawData::Split(&left[n..], right)
                } else {
                    let remainder_len = n - left.len();
                    if remainder_len > right.len() {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    RawData::Single(&right[remainder_len..])
                }
            }
        };
        Ok(())
    }

    #[inline]
    fn write_into(&self, target: &mut Vec<u8>) {
        target.clear();
        match *self {
            RawData::Single(slice) => target.extend_from_slice(slice),
            RawData::Split(first, second) => {
                target.reserve(first.len() + second.len());
                target.extend_from_slice(first);
                target.extend_from_slice(second);
            }
        }
    }

    pub fn as_slice(&self) -> Cow<'a, [u8]> {
        match *self {
            RawData::Single(buffer) => buffer.into(),
            RawData::Split(..) => {
                let mut vec = Vec::new();
                self.write_into(&mut vec);
                vec.into()
            }
        }
    }

    pub fn get(&self, range: Range<usize>) -> Option<RawData<'a>> {
        Some(match self {
            RawData::Single(buffer) => RawData::Single(buffer.get(range)?),
            RawData::Split(left, right) => {
                if range.start >= left.len() {
                    RawData::Single(right.get(range.start - left.len()..range.end - left.len())?)
                } else if range.end <= left.len() {
                    RawData::Single(left.get(range)?)
                } else {
                    let left = left.get(range.start..)?;
                    let right = right.get(..min(range.end - left.len(), right.len()))?;
                    RawData::Split(left, right)
                }
            }
        })
    }

    pub fn len(&self) -> usize {
        match *self {
            RawData::Single(buffer) => buffer.len(),
            RawData::Split(left, right) => left.len() + right.len(),
        }
    }
}

pub struct RawRegs<'a> {
    raw_data: RawData<'a>,
}

impl<'a> RawRegs<'a> {
    #[inline]
    pub(crate) fn from_raw_data(raw_data: RawData<'a>) -> Self {
        RawRegs { raw_data }
    }

    pub fn len(&self) -> usize {
        self.raw_data.len() / mem::size_of::<u64>()
    }

    // TODO: This should return an Option<u64>
    pub fn get(&self, index: usize) -> u64 {
        let offset = index * mem::size_of::<u64>();
        match self
            .raw_data
            .get(offset..offset + mem::size_of::<u64>())
            .unwrap()
        {
            RawData::Single(single) => NativeEndian::read_u64(single),
            RawData::Split(left, right) => {
                let mut buffer = [0; 4];
                let mut index = 0;
                for &byte in left {
                    buffer[index] = byte;
                    index += 1;
                }
                for &byte in right {
                    buffer[index] = byte;
                    index += 1;
                }

                NativeEndian::read_u64(&buffer)
            }
        }
    }
}

impl<'a> fmt::Debug for RawRegs<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let mut list = fmt.debug_list();
        for index in 0..self.len() {
            let value = self.get(index);
            list.entry(&HexValue(value));
        }

        list.finish()
    }
}
