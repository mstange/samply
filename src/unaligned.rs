use std::fmt::Debug;

use zerocopy::{FromBytes, Unaligned};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    LittleEndian,
    BigEndian,
}

/// An unaligned `u64` value with runtime endian.
#[derive(
    Unaligned, FromBytes, Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[repr(transparent)]
pub struct U64([u8; 8]);

impl U64 {
    pub fn get(&self, endian: Endianness) -> u64 {
        match endian {
            Endianness::LittleEndian => u64::from_le_bytes(self.0),
            Endianness::BigEndian => u64::from_be_bytes(self.0),
        }
    }
}

/// An unaligned `u32` value with runtime endian.
#[derive(
    Unaligned, FromBytes, Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[repr(transparent)]
pub struct U32([u8; 4]);

impl U32 {
    pub fn get(&self, endian: Endianness) -> u32 {
        match endian {
            Endianness::LittleEndian => u32::from_le_bytes(self.0),
            Endianness::BigEndian => u32::from_be_bytes(self.0),
        }
    }
}

/// An unaligned `u16` value with runtime endian.
#[derive(
    Unaligned, FromBytes, Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[repr(transparent)]
pub struct U16([u8; 2]);

impl U16 {
    pub fn get(&self, endian: Endianness) -> u16 {
        match endian {
            Endianness::LittleEndian => u16::from_le_bytes(self.0),
            Endianness::BigEndian => u16::from_be_bytes(self.0),
        }
    }
}
