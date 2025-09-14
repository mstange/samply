use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolMapGeneration(pub(crate) NonZeroU32);

static SYMBOL_MAP_GENERATION: AtomicU32 = AtomicU32::new(1);

impl Default for SymbolMapGeneration {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolMapGeneration {
    pub fn new() -> Self {
        let next_nonzero_wrapping = loop {
            match NonZeroU32::new(SYMBOL_MAP_GENERATION.fetch_add(1, Ordering::Relaxed)) {
                Some(gen) => break gen,
                None => continue,
            }
        };
        Self(next_nonzero_wrapping)
    }
}
