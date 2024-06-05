use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use fxhash::FxHasher;
use linux_perf_data::linux_perf_event_reader;
use linux_perf_event_reader::constants::{
    PERF_CONTEXT_GUEST, PERF_CONTEXT_GUEST_KERNEL, PERF_CONTEXT_GUEST_USER, PERF_CONTEXT_KERNEL,
    PERF_CONTEXT_USER,
};
use linux_perf_event_reader::CpuMode;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StackMode {
    User,
    Kernel,
}

impl StackMode {
    /// Detect stack mode from a "context frame".
    ///
    /// Context frames are present in sample callchains; they're u64 addresses
    /// which are `>= PERF_CONTEXT_MAX`.
    pub fn from_context_frame(frame: u64) -> Option<Self> {
        match frame {
            PERF_CONTEXT_KERNEL | PERF_CONTEXT_GUEST_KERNEL => Some(Self::Kernel),
            PERF_CONTEXT_USER | PERF_CONTEXT_GUEST | PERF_CONTEXT_GUEST_USER => Some(Self::User),
            _ => None,
        }
    }
}

impl From<CpuMode> for StackMode {
    /// Convert CpuMode into StackMode.
    fn from(cpu_mode: CpuMode) -> Self {
        match cpu_mode {
            CpuMode::Kernel | CpuMode::GuestKernel => Self::Kernel,
            _ => Self::User,
        }
    }
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
