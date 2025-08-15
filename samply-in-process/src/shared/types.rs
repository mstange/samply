pub type FastHashMap<K, V> = rustc_hash::FxHashMap<K, V>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StackMode {
    User,
    Kernel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StackFrame {
    InstructionPointer(u64, StackMode),
    ReturnAddress(u64, StackMode),
    AdjustedReturnAddress(u64, StackMode),
    TruncatedStackMarker,
}

impl StackFrame {
    pub fn stack_mode(&self) -> Option<StackMode> {
        match self {
            StackFrame::InstructionPointer(_, stack_mode) => Some(*stack_mode),
            StackFrame::ReturnAddress(_, stack_mode) => Some(*stack_mode),
            StackFrame::AdjustedReturnAddress(_, stack_mode) => Some(*stack_mode),
            StackFrame::TruncatedStackMarker => None,
        }
    }
}
