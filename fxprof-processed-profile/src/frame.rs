use bitflags::bitflags;

use crate::category::CategoryPairHandle;
use crate::global_lib_table::LibraryHandle;
use crate::profile::StringHandle;

/// A part of the information about a single stack frame.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub enum Frame {
    /// A code address taken from the instruction pointer
    InstructionPointer(u64),
    /// A code address taken from a return address
    ReturnAddress(u64),
    /// A relative address taken from the instruction pointer which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromInstructionPointer(LibraryHandle, u32),
    /// A relative address taken from a return address which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromReturnAddress(LibraryHandle, u32),
    /// A string, containing an index returned by Profile::intern_string
    Label(StringHandle),
}

/// All the information about a single stack frame.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub struct FrameInfo {
    /// The absolute address or label of this frame.
    pub frame: Frame,
    /// The category pair of this frame.
    pub category_pair: CategoryPairHandle,
    /// The flags of this frame. Use `FrameFlags::empty()` if unsure.
    pub flags: FrameFlags,
}

bitflags! {
    /// Flags for a stack frame.
    pub struct FrameFlags: u32 {
        /// Set on frames which are JavaScript functions.
        const IS_JS = 0b00000001;

        /// Set on frames which are not strictly JavaScript functions but which
        /// should be included in the JS-only call tree, such as DOM API calls.
        const IS_RELEVANT_FOR_JS = 0b00000010;
    }
}
