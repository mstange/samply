use std::iter::Peekable;

use fxprof_processed_profile::{
    LibMappings, LibraryHandle, SourceLocation, StringHandle, SubcategoryHandle,
};

use super::jit_category_manager::JsFrame;

#[derive(Debug, Clone)]
pub struct LibMappingInfo {
    pub lib_handle: LibraryHandle,
    pub category: Option<SubcategoryHandle>,
    pub js_frame: Option<JsFrame>,
    pub art_info: Option<AndroidArtInfo>,
    pub symbol: Option<JitSymbolInfo>,
}

/// The native symbol for a function in a synthetic JIT library.
///
/// JIT code has no symbol file to symbolicate against, so we know the symbol at
/// the time we see the function, and we put it on the frame directly rather than
/// going through a lib symbol table.
#[derive(Debug, Clone, Copy)]
pub struct JitSymbolInfo {
    pub lib_handle: LibraryHandle,
    pub name: StringHandle,
    /// The symbol's start address, relative to the synthetic JIT library.
    pub symbol_address: u32,
    pub symbol_size: Option<u32>,
    /// Source information for the native frames of this function.
    ///
    /// A prepended JS label frame carries its own, richer source location; this
    /// one is just for the native frame.
    pub source_location: SourceLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidArtInfo {
    /// Set when the lib mapping is for `libart.so`.
    LibArt,
    /// Set on a Java / Kotlin frame. This frame could come from a .dex / .vdex file,
    /// or from an .oat file, or it could be a JITted frame, or it could be a synthetic
    /// frame inserted by the interpreter.
    JavaFrame,
}

impl LibMappingInfo {
    pub fn new_lib(lib_handle: LibraryHandle) -> Self {
        Self {
            lib_handle,
            category: None,
            js_frame: None,
            art_info: None,
            symbol: None,
        }
    }

    #[allow(unused)]
    pub fn new_lib_with_category(lib_handle: LibraryHandle, category: SubcategoryHandle) -> Self {
        Self {
            lib_handle,
            category: Some(category),
            js_frame: None,
            art_info: None,
            symbol: None,
        }
    }

    pub fn new_jit_function(
        lib_handle: LibraryHandle,
        category: SubcategoryHandle,
        js_frame: Option<JsFrame>,
    ) -> Self {
        Self {
            lib_handle,
            category: Some(category),
            js_frame,
            art_info: None,
            symbol: None,
        }
    }

    pub fn new_libart_mapping(lib_handle: LibraryHandle) -> Self {
        Self {
            lib_handle,
            category: None,
            js_frame: None,
            art_info: Some(AndroidArtInfo::LibArt),
            symbol: None,
        }
    }

    pub fn new_java_mapping(
        lib_handle: LibraryHandle,
        category: Option<SubcategoryHandle>,
    ) -> Self {
        Self {
            lib_handle,
            category,
            js_frame: None,
            art_info: Some(AndroidArtInfo::JavaFrame),
            symbol: None,
        }
    }

    /// Attach the native symbol for a function in a synthetic JIT library, so
    /// that its frames don't need a lib symbol table to get their name.
    pub fn with_jit_symbol(mut self, symbol: JitSymbolInfo) -> Self {
        self.symbol = Some(symbol);
        self
    }
}

#[derive(Debug)]
pub struct LibMappingsHierarchy {
    regular_libs: (LibMappings<LibMappingInfo>, LibMappingOpQueueIter),
    jitdumps: Vec<(LibMappings<LibMappingInfo>, LibMappingOpQueueIter)>,
    perf_map: Option<LibMappings<LibMappingInfo>>,
}

impl LibMappingsHierarchy {
    pub fn new(regular_lib_mappings_ops: LibMappingOpQueue) -> Self {
        Self {
            regular_libs: (LibMappings::default(), regular_lib_mappings_ops.into_iter()),
            jitdumps: Vec::new(),
            perf_map: None,
        }
    }

    pub fn add_jitdump_lib_mappings_ops(&mut self, lib_mappings_ops: LibMappingOpQueue) {
        self.jitdumps
            .push((LibMappings::default(), lib_mappings_ops.into_iter()));
    }

    pub fn add_perf_map_mappings(&mut self, mappings: LibMappings<LibMappingInfo>) {
        self.perf_map = Some(mappings);
    }

    pub fn process_ops(&mut self, timestamp: u64) {
        while let Some(op) = self.regular_libs.1.next_op_if_at_or_before(timestamp) {
            op.apply_to(&mut self.regular_libs.0);
        }
        for (mappings, ops) in &mut self.jitdumps {
            while let Some(op) = ops.next_op_if_at_or_before(timestamp) {
                op.apply_to(mappings);
            }
        }
    }

    pub fn convert_address(&self, address: u64) -> Option<(u32, &LibMappingInfo)> {
        if let Some(x) = self.regular_libs.0.convert_address(address) {
            return Some(x);
        }
        for (mappings, _ops) in &self.jitdumps {
            if let Some(x) = mappings.convert_address(address) {
                return Some(x);
            }
        }
        if let Some(perf_map) = &self.perf_map {
            if let Some(x) = perf_map.convert_address(address) {
                return Some(x);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct LibMappingOpQueue(Vec<(u64, LibMappingOp)>);

impl LibMappingOpQueue {
    pub fn push(&mut self, timestamp: u64, op: LibMappingOp) {
        self.0.push((timestamp, op));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_iter(self) -> LibMappingOpQueueIter {
        LibMappingOpQueueIter(self.0.into_iter().peekable())
    }
}

#[derive(Debug)]
pub struct LibMappingOpQueueIter(Peekable<std::vec::IntoIter<(u64, LibMappingOp)>>);

impl LibMappingOpQueueIter {
    pub fn next_op_if_at_or_before(&mut self, timestamp: u64) -> Option<LibMappingOp> {
        if self.0.peek()?.0 > timestamp {
            return None;
        }
        let (_timestamp, op) = self.0.next().unwrap();
        Some(op)
    }
}

#[derive(Debug, Clone)]
pub enum LibMappingOp {
    Add(LibMappingAdd),
    Move(LibMappingMove),
    #[allow(unused)]
    Remove(LibMappingRemove),
    Clear,
}

impl LibMappingOp {
    pub fn apply_to(self, lib_mappings: &mut LibMappings<LibMappingInfo>) {
        match self {
            LibMappingOp::Add(op) => {
                lib_mappings.add_mapping(
                    op.start_avma,
                    op.end_avma,
                    op.relative_address_at_start,
                    op.info,
                );
            }
            LibMappingOp::Move(op) => {
                if let Some((relative_address_at_start, info)) =
                    lib_mappings.remove_mapping(op.old_start_avma)
                {
                    lib_mappings.add_mapping(
                        op.new_start_avma,
                        op.new_end_avma,
                        relative_address_at_start,
                        info,
                    );
                }
            }
            LibMappingOp::Remove(op) => {
                lib_mappings.remove_mapping(op.start_avma);
            }
            LibMappingOp::Clear => {
                lib_mappings.clear();
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct LibMappingAdd {
    pub start_avma: u64,
    pub end_avma: u64,
    pub relative_address_at_start: u32,
    pub info: LibMappingInfo,
}

#[derive(Debug, Clone)]
pub struct LibMappingMove {
    pub old_start_avma: u64,
    pub new_start_avma: u64,
    pub new_end_avma: u64,
}

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct LibMappingRemove {
    pub start_avma: u64,
}
