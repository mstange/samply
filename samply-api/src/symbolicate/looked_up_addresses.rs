use std::collections::BTreeMap;

use samply_symbols::{FrameDebugInfo, SourceFilePath, SourceFilePathHandle, SymbolInfo};

pub trait PathResolver {
    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_>;
}

impl PathResolver for () {
    fn resolve_source_file_path(&self, _handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        unreachable!()
    }
}

pub struct AddressResult {
    pub symbol: SymbolInfo,
    pub inline_frames: Option<Vec<FrameDebugInfo>>,
}

impl AddressResult {
    pub fn new(symbol: SymbolInfo) -> Self {
        Self {
            symbol,
            inline_frames: None,
        }
    }

    pub fn set_debug_info(&mut self, frames: Vec<FrameDebugInfo>) {
        let outer_function_name = frames.last().and_then(|f| f.function.as_ref());
        // Overwrite the symbol name with the function name from the debug info.
        if let Some(name) = outer_function_name {
            self.symbol.name = name.clone();
        }
        // Add the inline frame info.
        self.inline_frames = Some(frames);
    }
}

pub type AddressResults = BTreeMap<u32, Option<AddressResult>>;
