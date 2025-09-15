use std::collections::BTreeMap;

use samply_symbols::{FrameDebugInfo, SourceFilePath, SourceFilePathHandle};

pub trait PathResolver {
    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_>;
}

impl PathResolver for () {
    fn resolve_source_file_path(&self, _handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        unreachable!()
    }
}

pub struct AddressResult {
    pub symbol_address: u32,
    pub symbol_name: String,
    pub function_size: Option<u32>,
    pub inline_frames: Option<Vec<FrameDebugInfo>>,
}

impl AddressResult {
    pub fn new(symbol_address: u32, symbol_name: String, function_size: Option<u32>) -> Self {
        Self {
            symbol_address,
            symbol_name,
            function_size,
            inline_frames: None,
        }
    }

    pub fn set_debug_info(&mut self, frames: Vec<FrameDebugInfo>) {
        let outer_function_name = frames.last().and_then(|f| f.function.as_deref());
        // Overwrite the symbol name with the function name from the debug info.
        if let Some(name) = outer_function_name {
            self.symbol_name = name.to_string();
        }
        // Add the inline frame info.
        self.inline_frames = Some(frames);
    }
}

pub type AddressResults = BTreeMap<u32, Option<AddressResult>>;
