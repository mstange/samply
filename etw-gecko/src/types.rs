use fxhash::FxHasher;

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StackMode {
    User,
    Kernel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StackFrame {
    InstructionPointer(u64, StackMode),
    ReturnAddress(u64, StackMode),
    TruncatedStackMarker,
}
