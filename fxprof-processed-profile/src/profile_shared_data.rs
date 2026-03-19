use std::collections::BTreeMap;

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::fast_hash_map::FastHashSet;
use crate::frame_table::{FrameInterner, InternalFrame};
use crate::global_lib_table::{GlobalLibIndex, UsedLibraryAddressesCollector};
use crate::native_symbols::{NativeSymbolIndex, NativeSymbols};
use crate::profile_symbol_info::LibSymbolInfo;
use crate::stack_table::StackTable;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::symbol_info::SymbolStringTable;
use crate::symbolication::{apply_symbol_information, StringTableAdapter};
use crate::{FrameHandle, StackHandle};

#[derive(Debug)]
pub struct ProfileSharedData {
    pub(crate) string_table: ProfileStringTable,
    stack_table: StackTable,
    frame_interner: FrameInterner,
    pub(crate) native_symbols: NativeSymbols,
}

impl ProfileSharedData {
    pub fn new() -> Self {
        Self {
            string_table: ProfileStringTable::new(),
            stack_table: StackTable::new(),
            frame_interner: FrameInterner::new(),
            native_symbols: NativeSymbols::new(),
        }
    }

    pub fn get_native_symbol_name(&self, native_symbol_index: NativeSymbolIndex) -> StringHandle {
        self.native_symbols
            .get_native_symbol_name(native_symbol_index)
    }

    pub fn frame_index_for_frame(&mut self, frame: InternalFrame) -> FrameHandle {
        self.frame_interner.index_for_frame(frame)
    }

    pub fn stack_index_for_stack(
        &mut self,
        prefix: Option<StackHandle>,
        frame: FrameHandle,
    ) -> StackHandle {
        self.stack_table.index_for_stack(prefix, frame)
    }

    pub fn contains_js_frame(&self) -> bool {
        self.frame_interner.contains_js_frame()
    }

    pub fn gather_used_rvas(&self, collector: &mut UsedLibraryAddressesCollector) {
        self.frame_interner.gather_used_rvas(collector);
    }

    pub fn make_symbolicated_shared(
        self,
        libs: &FastHashSet<GlobalLibIndex>,
        lib_symbols: &BTreeMap<GlobalLibIndex, &LibSymbolInfo>,
        symbol_string_table: &SymbolStringTable,
    ) -> (ProfileSharedData, Vec<Option<StackHandle>>) {
        let ProfileSharedData {
            mut string_table,
            stack_table,
            frame_interner,
            native_symbols,
        } = self;

        let mut strings = StringTableAdapter::new(symbol_string_table, &mut string_table);
        let (frame_interner, native_symbols, stack_table, old_stack_to_new_stack) =
            apply_symbol_information(
                frame_interner,
                native_symbols,
                stack_table,
                libs,
                lib_symbols,
                &mut strings,
            );

        (
            ProfileSharedData {
                string_table,
                stack_table,
                frame_interner,
                native_symbols,
            },
            old_stack_to_new_stack,
        )
    }
}

impl Serialize for ProfileSharedData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (frame_table, func_table, source_table, resource_table) =
            self.frame_interner.create_tables();

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("stackTable", &self.stack_table)?;
        map.serialize_entry("frameTable", &frame_table)?;
        map.serialize_entry("funcTable", &func_table)?;
        map.serialize_entry("nativeSymbols", &self.native_symbols)?;
        map.serialize_entry("resourceTable", &resource_table)?;
        map.serialize_entry("sources", &source_table)?;
        map.serialize_entry("stringArray", &self.string_table)?;
        map.end()
    }
}
