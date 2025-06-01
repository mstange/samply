use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::ser::{SerializeMap, Serializer};

use crate::cpu_delta::CpuDelta;
use crate::fast_hash_map::FastHashMap;
use crate::frame_table::{FrameInterner, InternalFrame, InternalFrameVariant, NativeFrameData};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable, UsedLibraryAddressesCollector};
use crate::marker_table::MarkerTable;
use crate::markers::InternalMarkerSchema;
use crate::native_symbols::{NativeSymbolIndex, NativeSymbols};
use crate::profile_symbol_info::{
    AddressFrame, AddressInfo, FileStringIndex, FunctionNameStringIndex, ProfileLibSymbolInfo,
    ProfileSymbolInfo,
};
use crate::sample_table::{NativeAllocationsTable, SampleTable, WeightType};
use crate::stack_table::StackTable;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::{
    FrameFlags, LibraryHandle, Marker, MarkerHandle, MarkerTiming, MarkerTypeHandle,
    SourceLocation, SubcategoryHandle, Symbol, Timestamp,
};

/// A process. Can be created with [`Profile::add_process`](crate::Profile::add_process).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ProcessHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Thread {
    process: ProcessHandle,
    tid: String,
    name: Option<String>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    is_main: bool,
    stack_table: StackTable,
    frame_interner: FrameInterner,
    samples: SampleTable,
    native_allocations: Option<NativeAllocationsTable>,
    markers: MarkerTable,
    native_symbols: NativeSymbols,
    last_sample_stack: Option<usize>,
    last_sample_was_zero_cpu: bool,
    show_markers_in_timeline: bool,
}

impl Thread {
    pub fn new(process: ProcessHandle, tid: String, start_time: Timestamp, is_main: bool) -> Self {
        Self {
            process,
            tid,
            name: None,
            start_time,
            end_time: None,
            is_main,
            stack_table: StackTable::new(),
            frame_interner: FrameInterner::new(),
            samples: SampleTable::new(),
            native_allocations: None,
            markers: MarkerTable::new(),
            native_symbols: NativeSymbols::new(),
            last_sample_stack: None,
            last_sample_was_zero_cpu: false,
            show_markers_in_timeline: false,
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = Some(name.to_string());
    }

    pub fn set_start_time(&mut self, start_time: Timestamp) {
        self.start_time = start_time;
    }

    pub fn set_end_time(&mut self, end_time: Timestamp) {
        self.end_time = Some(end_time);
    }

    pub fn set_tid(&mut self, tid: String) {
        self.tid = tid;
    }

    pub fn set_show_markers_in_timeline(&mut self, v: bool) {
        self.show_markers_in_timeline = v;
    }

    pub fn process(&self) -> ProcessHandle {
        self.process
    }

    pub fn native_symbol_index_and_string_index_for_symbol(
        &mut self,
        lib_index: GlobalLibIndex,
        symbol: &Symbol,
        string_table: &mut ProfileStringTable,
    ) -> (NativeSymbolIndex, StringHandle) {
        self.native_symbols
            .symbol_index_and_string_index_for_symbol(lib_index, symbol, string_table)
    }

    pub fn native_symbol_index_for_native_symbol(
        &mut self,
        lib_index: GlobalLibIndex,
        symbol: &Symbol,
        string_table: &mut ProfileStringTable,
    ) -> NativeSymbolIndex {
        let (symbol_index, _) =
            self.native_symbol_index_and_string_index_for_symbol(lib_index, symbol, string_table);
        symbol_index
    }

    pub fn get_native_symbol_name(&self, native_symbol_index: NativeSymbolIndex) -> StringHandle {
        self.native_symbols
            .get_native_symbol_name(native_symbol_index)
    }

    pub fn frame_index_for_frame(&mut self, frame: InternalFrame) -> usize {
        self.frame_interner.index_for_frame(frame)
    }

    pub fn stack_index_for_stack(&mut self, prefix: Option<usize>, frame: usize) -> usize {
        self.stack_table.index_for_stack(prefix, frame)
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        self.samples
            .add_sample(timestamp, stack_index, cpu_delta, weight);
        self.last_sample_stack = stack_index;
        self.last_sample_was_zero_cpu = cpu_delta == CpuDelta::ZERO;
    }

    pub fn add_allocation_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        // Create allocations table, if it doesn't exist yet.
        let allocations = self.native_allocations.get_or_insert_with(Default::default);

        // Add the allocation sample.
        allocations.add_sample(timestamp, stack_index, allocation_address, allocation_size);
    }

    pub fn add_sample_same_stack_zero_cpu(&mut self, timestamp: Timestamp, weight: i32) {
        if self.last_sample_was_zero_cpu {
            self.samples.modify_last_sample(timestamp, weight);
        } else {
            let stack_index = self.last_sample_stack;
            self.samples
                .add_sample(timestamp, stack_index, CpuDelta::ZERO, weight);
            self.last_sample_was_zero_cpu = true;
        }
    }

    pub fn set_samples_weight_type(&mut self, t: WeightType) {
        self.samples.set_weight_type(t);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_marker<T: Marker>(
        &mut self,
        string_table: &mut ProfileStringTable,
        name_string_index: StringHandle,
        marker_type_handle: MarkerTypeHandle,
        schema: &InternalMarkerSchema,
        marker: T,
        timing: MarkerTiming,
    ) -> MarkerHandle {
        self.markers.add_marker(
            string_table,
            name_string_index,
            marker_type_handle,
            schema,
            marker,
            timing,
        )
    }

    pub fn set_marker_stack(&mut self, marker: MarkerHandle, stack_index: Option<usize>) {
        self.markers.set_marker_stack(marker, stack_index);
    }

    pub fn contains_js_frame(&self) -> bool {
        self.frame_interner.contains_js_frame()
    }

    pub fn gather_used_rvas(&self, collector: &mut UsedLibraryAddressesCollector) {
        self.frame_interner.gather_used_rvas(collector);
    }

    pub fn cmp_for_json_order(&self, other: &Thread) -> Ordering {
        let ordering = (!self.is_main).cmp(&(!other.is_main));
        if ordering != Ordering::Equal {
            return ordering;
        }
        if let Some(ordering) = self.start_time.partial_cmp(&other.start_time) {
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        let ordering = self.name.cmp(&other.name);
        if ordering != Ordering::Equal {
            return ordering;
        }
        self.tid.cmp(&other.tid)
    }

    pub fn make_symbolicated_thread(
        self,
        symbol_info: &ProfileSymbolInfo,
        global_libs: &GlobalLibTable,
        string_table: &ProfileStringTable,
    ) -> Thread {
        let ProfileSymbolInfo {
            function_names,
            files,
            lib_symbols,
        } = symbol_info;
        let lib_symbols: BTreeMap<GlobalLibIndex, &ProfileLibSymbolInfo> = lib_symbols
            .iter()
            .map(|lib_symbols| {
                let lib_index = global_libs.used_lib_index(lib_symbols.lib_handle).unwrap();
                (lib_index, lib_symbols)
            })
            .collect();
        let libs: BTreeSet<GlobalLibIndex> = lib_symbols.keys().cloned().collect();

        if libs.is_empty() {
            return self;
        }

        let Thread {
            process,
            tid,
            name,
            start_time,
            end_time,
            is_main,
            stack_table,
            frame_interner,
            samples,
            native_allocations,
            markers,
            native_symbols,
            last_sample_stack,
            last_sample_was_zero_cpu,
            show_markers_in_timeline,
        } = self;

        let frames = frame_interner.into_frames();
        let mut new_frame_interner = FrameInterner::new();

        let mut function_names_map: FastHashMap<FunctionNameStringIndex, StringHandle> =
            Default::default();
        let mut file_names_map: FastHashMap<FileStringIndex, StringHandle> = Default::default();

        let mut native_frames = BTreeSet::new();

        let (mut new_native_symbols, old_native_symbol_to_new_native_symbol) =
            native_symbols.new_table_with_symbols_from_libs_removed(&libs);

        enum StackNodeConversionAction {
            RemapIndex(usize),
            Symbolicate(FrameKeyForSymbolication),
            DiscardInlined,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        struct FrameKeyForSymbolication {
            lib: GlobalLibIndex,
            address: u32,
            subcategory: SubcategoryHandle,
            frame_flags: FrameFlags,
        }

        let conversion_action_for_stack_frame: Vec<StackNodeConversionAction> = frames
            .iter()
            .map(|frame| match frame.variant {
                InternalFrameVariant::Label => {
                    let new_frame_index = new_frame_interner.index_for_frame(*frame);
                    StackNodeConversionAction::RemapIndex(new_frame_index)
                }
                InternalFrameVariant::Native(NativeFrameData {
                    lib,
                    relative_address,
                    inline_depth,
                    native_symbol,
                }) => {
                    if libs.contains(&lib) {
                        if inline_depth == 0 {
                            let key = FrameKeyForSymbolication {
                                lib,
                                address: relative_address,
                                subcategory: frame.subcategory,
                                frame_flags: frame.flags,
                            };
                            native_frames.insert(key);
                            StackNodeConversionAction::Symbolicate(key)
                        } else {
                            StackNodeConversionAction::DiscardInlined
                        }
                    } else {
                        // This is a native frame, but we're not applying any symbols for its
                        // library, so just take it as-is and put it into new_frame_interner,
                        // but with an updated native_symbol.
                        let new_native_symbol =
                            native_symbol.map(|ns| old_native_symbol_to_new_native_symbol.map(ns));
                        let new_frame_index = new_frame_interner.index_for_frame(InternalFrame {
                            name: frame.name,
                            variant: InternalFrameVariant::Native(NativeFrameData {
                                lib,
                                relative_address,
                                inline_depth,
                                native_symbol: new_native_symbol,
                            }),
                            subcategory: frame.subcategory,
                            source_location: frame.source_location,
                            flags: frame.flags,
                        });
                        StackNodeConversionAction::RemapIndex(new_frame_index)
                    }
                }
            })
            .collect();

        let mut symbolicated_frames_by_key = BTreeMap::new();
        if let Some(first_frame) = native_frames.iter().next() {
            let mut current_lib = first_frame.lib;
            let mut current_lib_symbols = lib_symbols.get(&current_lib).unwrap();
            let mut current_lib_next_address_index = 0;
            for frame_key in native_frames {
                let FrameKeyForSymbolication {
                    lib,
                    address,
                    subcategory,
                    frame_flags,
                } = frame_key;

                if lib != current_lib {
                    current_lib = lib;
                    current_lib_symbols = lib_symbols.get(&current_lib).unwrap();
                    current_lib_next_address_index = 0;
                }

                while current_lib_next_address_index < current_lib_symbols.sorted_addresses.len()
                    && current_lib_symbols.sorted_addresses[current_lib_next_address_index]
                        < address
                {
                    current_lib_next_address_index += 1;
                }

                let outer_frame_index;
                let frame_count;

                if current_lib_next_address_index < current_lib_symbols.sorted_addresses.len()
                    && current_lib_symbols.sorted_addresses[current_lib_next_address_index]
                        == address
                {
                    // Found!
                    let address_info =
                        &current_lib_symbols.address_infos[current_lib_next_address_index];
                    let AddressInfo {
                        symbol_name,
                        symbol_start_address,
                        symbol_size,
                        ref frames,
                    } = *address_info;
                    let symbol_name_string_index =
                        *function_names_map.entry(symbol_name).or_insert_with(|| {
                            string_table.index_for_string(
                                function_names.0.get_string(symbol_name.0).unwrap(),
                            )
                        });
                    let native_symbol_index = new_native_symbols.symbol_index_for_symbol(
                        lib,
                        symbol_start_address,
                        symbol_size,
                        symbol_name_string_index,
                    );

                    if frames.is_empty() {
                        // Outer function uses symbol_name
                        outer_frame_index = new_frame_interner.index_for_frame(InternalFrame {
                            name: symbol_name_string_index,
                            variant: InternalFrameVariant::Native(NativeFrameData {
                                lib,
                                native_symbol: Some(native_symbol_index),
                                relative_address: address,
                                inline_depth: 0,
                            }),
                            subcategory,
                            source_location: SourceLocation::default(),
                            flags: frame_flags,
                        });
                        frame_count = 1;
                    } else {
                        let mut new_frame_indexes = Vec::with_capacity(frames.len());
                        for (inline_depth, frame) in frames.into_iter().enumerate() {
                            // create one InternalFrame each
                            let AddressFrame {
                                function_name,
                                file,
                                line,
                            } = *frame;
                            let function_name_string_index =
                                *function_names_map.entry(function_name).or_insert_with(|| {
                                    string_table.index_for_string(
                                        function_names.0.get_string(function_name.0).unwrap(),
                                    )
                                });
                            let file_name_string_index =
                                *file_names_map.entry(file).or_insert_with(|| {
                                    string_table
                                        .index_for_string(files.0.get_string(file.0).unwrap())
                                });
                            let new_frame_index =
                                new_frame_interner.index_for_frame(InternalFrame {
                                    name: function_name_string_index,
                                    variant: InternalFrameVariant::Native(NativeFrameData {
                                        lib,
                                        native_symbol: Some(native_symbol_index),
                                        relative_address: address,
                                        inline_depth: inline_depth as u16,
                                    }),
                                    subcategory,
                                    source_location: SourceLocation {
                                        file_path: Some(file_name_string_index),
                                        line,
                                        col: None,
                                    },
                                    flags: frame_flags,
                                });
                            new_frame_indexes.push(new_frame_index);
                        }
                        outer_frame_index = new_frame_indexes[0];
                        frame_count = new_frame_indexes.len();
                        // We use a compact representation of the new frames: (start index, len)
                        // Examples:
                        // - (5, 1) represents the sequence [5] (len 1)
                        // - (3, 4) represents the sequence [3, 4, 5, 6] (len 4)
                        // This only works if the new indexes are consecutive, which they should be.
                        // Every frame should be new to new_frame_interner because it should be
                        // a combination of (lib, address, inline depth) that the interner hasn't
                        // seen before - it didn't contain any frames at all from lib before the
                        // outer loop.
                        assert_eq!(
                            *new_frame_indexes.last().unwrap(),
                            outer_frame_index + frame_count - 1,
                            "Every frame we're creating here should be a new frame, and they should have consecutive indexes."
                        );
                    }
                } else {
                    // Not found
                    let name = string_table.index_for_hex_address_string(address.into());
                    outer_frame_index = new_frame_interner.index_for_frame(InternalFrame {
                        name,
                        variant: InternalFrameVariant::Native(NativeFrameData {
                            lib,
                            native_symbol: None,
                            relative_address: address,
                            inline_depth: 0,
                        }),
                        subcategory,
                        source_location: SourceLocation::default(),
                        flags: frame_flags,
                    });
                    frame_count = 1;
                }

                symbolicated_frames_by_key.insert(frame_key, (outer_frame_index, frame_count));
            }
        }

        let mut new_stack_table = StackTable::new();
        let mut old_stack_to_new_stack = Vec::with_capacity(stack_table.index_for_stack(prefix, frame));

        todo!("do something about the stack table and about mapping old stacks to new stacks");
        // Go through the stack table. For each stack node:
        // Translate prefix using old_stack_to_new_stack. Then:
        // Check conversion_action_for_stack_frame[old_frame_index]:
        // - RemapIndex:
        //   - create stack node with new indexes
        //   - old_stack_to_new_stack[old_stack] = new_stack
        // - Symbolicate:
        //   - look up (outer_frame_index, frame_count) in symbolicated_frames_by_lib_and_address
        //   - create frame_count new stack nodes. outer node has translated prefix,
        //     each next node has the just-created stack as its parent
        //   - old_stack_to_new_stack[old_stack] = new_stack for innermost frame
        // - DiscardInlined:
        //   - old_stack_to_new_stack[old_stack] = translated prefix

        todo!("remap stack indexes in samples and markers");

        Thread {
            process,
            tid,
            name,
            start_time,
            end_time,
            is_main,
            stack_table,
            frame_interner,
            samples,
            native_allocations,
            markers,
            native_symbols,
            last_sample_stack,
            last_sample_was_zero_cpu,
            show_markers_in_timeline,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn serialize_with<S: Serializer>(
        &self,
        serializer: S,
        process_start_time: Timestamp,
        process_end_time: Option<Timestamp>,
        process_name: &str,
        pid: &str,
        marker_schemas: &[InternalMarkerSchema],
        string_table: &ProfileStringTable,
    ) -> Result<S::Ok, S::Error> {
        let thread_name: Cow<str> = match (self.is_main, &self.name) {
            (true, _) => process_name.into(),
            (false, Some(name)) => name.into(),
            (false, None) => format!("Thread <{}>", self.tid).into(),
        };

        let thread_register_time = self.start_time;
        let thread_unregister_time = self.end_time;

        let (frame_table, func_table, resource_table) = self.frame_interner.create_tables();

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("frameTable", &frame_table)?;
        map.serialize_entry("funcTable", &func_table)?;
        map.serialize_entry(
            "markers",
            &self.markers.as_serializable(marker_schemas, string_table),
        )?;
        map.serialize_entry("name", &thread_name)?;
        map.serialize_entry("isMainThread", &self.is_main)?;
        map.serialize_entry("nativeSymbols", &self.native_symbols)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("pid", &pid)?;
        map.serialize_entry("processName", process_name)?;
        map.serialize_entry("processShutdownTime", &process_end_time)?;
        map.serialize_entry("processStartupTime", &process_start_time)?;
        map.serialize_entry("processType", &"default")?;
        map.serialize_entry("registerTime", &thread_register_time)?;
        map.serialize_entry("resourceTable", &resource_table)?;
        map.serialize_entry("samples", &self.samples)?;
        if let Some(allocations) = &self.native_allocations {
            map.serialize_entry("nativeAllocations", &allocations)?;
        }
        map.serialize_entry("stackTable", &self.stack_table)?;
        map.serialize_entry("tid", &self.tid)?;
        map.serialize_entry("unregisterTime", &thread_unregister_time)?;
        map.serialize_entry("showMarkersInTimeline", &self.show_markers_in_timeline)?;
        map.end()
    }
}
