use std::sync::atomic::AtomicU32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolMapGeneration(pub(crate) u32);

static SYMBOL_MAP_GENERATION: AtomicU32 = AtomicU32::new(0);

impl Default for SymbolMapGeneration {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolMapGeneration {
    pub fn new() -> Self {
        Self(SYMBOL_MAP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}
