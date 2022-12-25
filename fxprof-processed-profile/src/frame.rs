use crate::profile::StringHandle;

/// A single stack frame.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub enum Frame {
    /// A code address taken from the instruction pointer
    InstructionPointer(u64),
    /// A code address taken from a return address
    ReturnAddress(u64),
    /// A string, containing an index returned by Profile::intern_string
    Label(StringHandle),
}
