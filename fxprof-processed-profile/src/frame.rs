use bitflags::bitflags;

use crate::category::CategoryPairHandle;
use crate::global_lib_table::LibraryHandle;
use crate::profile::StringHandle;

/// A part of the information about a single stack frame.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub enum Frame {
    /// A code address taken from the instruction pointer.
    ///
    /// This code address will be resolved to a library-relative address using
    /// the library mappings on the process which were specified using
    /// [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
    InstructionPointer(u64),
    /// A code address taken from a return address
    ///
    /// This code address will be resolved to a library-relative address using
    /// the library mappings on the process which were specified using
    /// [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
    ReturnAddress(u64),
    /// A code address taken from a return address, but adjusted so that it
    /// points into the previous instruction. Usually this is "return address
    /// minus one byte", but some unwinders subtract 2 or 4 bytes if they know
    /// more about the architecture-dependent instruction size.
    ///
    /// When you call a function with a call instruction, the return address
    /// is set up in such a way that, once the called function returns, the CPU
    /// continues executing after the call instruction. That means that the return
    /// address points to the instruction *after* the call instruction. But for
    /// stack unwinding, you're interested in **the call instruction itself**.
    /// The call instruction and the instruction after often have very different
    /// symbol information (different line numbers, or even different inline stacks).
    ///
    /// This code address will be resolved to a library-relative address using
    /// the library mappings on the process which were specified using
    /// [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
    AdjustedReturnAddress(u64),
    /// A relative address taken from the instruction pointer which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromInstructionPointer(LibraryHandle, u32),
    /// A relative address taken from a return address which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromReturnAddress(LibraryHandle, u32),
    /// A relative address taken from an adjusted return address which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromAdjustedReturnAddress(LibraryHandle, u32),
    /// A string, containing an index returned by
    /// [`Profile::intern_string`](crate::Profile::intern_string).
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
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
    pub struct FrameFlags: u32 {
        /// Set on frames which are JavaScript functions.
        const IS_JS = 0b00000001;

        /// Set on frames which are not strictly JavaScript functions but which
        /// should be included in the JS-only call tree, such as DOM API calls.
        const IS_RELEVANT_FOR_JS = 0b00000010;
    }
}
