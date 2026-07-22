use bitflags::bitflags;

use crate::{global_lib_table::LibraryHandle, ProcessHandle};

/// The address information of a stack frame.
///
/// There are three groups of enum variants, based on how the address gets
/// resolved to a library:
///
/// 1. Process addresses (`InstructionPointer`, `ReturnAddress`,
///    `AdjustedReturnAddress`): absolute (process-relative) addresses that are
///    resolved against the per-process library mappings configured via
///    [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
/// 2. Kernel addresses (`KernelInstructionPointer`, `KernelReturnAddress`,
///    `KernelAdjustedReturnAddress`): absolute addresses in the kernel's
///    address space, resolved against the global kernel mappings configured via
///    [`Profile::add_kernel_lib_mapping`](crate::Profile::add_kernel_lib_mapping).
/// 3. Pre-resolved relative addresses
///    (`RelativeAddressFromInstructionPointer`, `RelativeAddressFromReturnAddress`,
///    `RelativeAddressFromAdjustedReturnAddress`): the caller has already
///    mapped the address to a [`LibraryHandle`] and a library-relative offset.
///    Use these when you do your own address resolution.
///
/// Within each group, the choice between `InstructionPointer`, `ReturnAddress`,
/// and `AdjustedReturnAddress` determines whether symbol lookup will subtract one
/// byte or not.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub enum FrameAddress {
    /// A code address taken from the instruction pointer.
    ///
    /// This code address will be resolved to a library-relative address using
    /// the library mappings on the process which were specified using
    /// [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
    InstructionPointer(ProcessHandle, u64),
    /// A code address taken from a return address
    ///
    /// This code address will be resolved to a library-relative address using
    /// the library mappings on the process which were specified using
    /// [`Profile::add_lib_mapping`](crate::Profile::add_lib_mapping).
    ReturnAddress(ProcessHandle, u64),
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
    AdjustedReturnAddress(ProcessHandle, u64),

    /// A code address taken from a kernel instruction pointer.
    ///
    /// Resolved using the global kernel mappings configured via
    /// [`Profile::add_kernel_lib_mapping`](crate::Profile::add_kernel_lib_mapping).
    /// Unlike the per-process variants above, this is not associated with a
    /// specific [`ProcessHandle`] because the kernel address space is global.
    KernelInstructionPointer(u64),
    /// A code address taken from a kernel return address.
    ///
    /// Resolved using the global kernel mappings configured via
    /// [`Profile::add_kernel_lib_mapping`](crate::Profile::add_kernel_lib_mapping).
    KernelReturnAddress(u64),
    /// A code address taken from a kernel return address, adjusted to point
    /// into the previous instruction. See [`FrameAddress::AdjustedReturnAddress`]
    /// for the rationale.
    ///
    /// Resolved using the global kernel mappings configured via
    /// [`Profile::add_kernel_lib_mapping`](crate::Profile::add_kernel_lib_mapping).
    KernelAdjustedReturnAddress(u64),

    /// A relative address taken from the instruction pointer which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromInstructionPointer(LibraryHandle, u32),
    /// A relative address taken from a return address which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromReturnAddress(LibraryHandle, u32),
    /// A relative address taken from an adjusted return address which
    /// has already been resolved to a `LibraryHandle`.
    RelativeAddressFromAdjustedReturnAddress(LibraryHandle, u32),
}

bitflags! {
    /// Flags for a stack frame.
    ///
    /// Native frames almost always use [`FrameFlags::empty()`]. Combine flags
    /// with the bitwise `|` operator:
    ///
    /// ```
    /// use fxprof_processed_profile::FrameFlags;
    /// let flags = FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS;
    /// ```
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
    pub struct FrameFlags: u32 {
        /// Set on frames which are JavaScript functions.
        const IS_JS = 0b00000001;

        /// Set on frames which are not strictly JavaScript functions but which
        /// should be included in the JS-only call tree, such as DOM API calls.
        const IS_RELEVANT_FOR_JS = 0b00000010;
    }
}
