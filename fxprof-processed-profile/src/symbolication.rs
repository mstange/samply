use std::collections::{btree_set, BTreeMap, BTreeSet};

use crate::fast_hash_map::{FastHashMap, FastHashSet};
use crate::frame_table::{FrameInterner, InternalFrame, InternalFrameVariant, NativeFrameData};
use crate::global_lib_table::GlobalLibIndex;
use crate::native_symbols::{NativeSymbolIndexTranslator, NativeSymbols};
use crate::profile_symbol_info::{
    AddressInfo, LibSymbolInfo, SymbolStringIndex, SymbolStringTable,
};
use crate::stack_table::StackTable;
use crate::string_table::ProfileStringTable;
use crate::{FrameFlags, SourceLocation, StringHandle, SubcategoryHandle};

/// Describes a native frame which should be replaced with its symbolicated
/// equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameKeyForSymbolication {
    pub lib: GlobalLibIndex,
    pub address: u32,
    pub subcategory: SubcategoryHandle,
    pub frame_flags: FrameFlags,
}

impl FrameKeyForSymbolication {
    pub fn new(old_frame: &InternalFrame, native: &NativeFrameData) -> Self {
        Self {
            lib: native.lib,
            address: native.relative_address,
            subcategory: old_frame.subcategory,
            frame_flags: old_frame.flags,
        }
    }
}

/// Describes what to do when creating the new [`StackTable`], based on the
/// stack node's frame.
pub enum StackNodeConversionAction {
    /// Change the frame index to the new index.
    RemapIndex(usize),
    /// Replace this stack node with one or more stack nodes by looking up
    /// the symbolicated frames for this frame key.
    Symbolicate(FrameKeyForSymbolication),
    /// Discard this stack node, i.e. reparent our children to our parent,
    /// because this node was referring to a frame with inline_depth > 0.
    /// If the symbol information for a frame included inlined frames, we
    /// created their corresponding stack nodes when we encountered the
    /// stack node for the inline_depth==0 frame, so any old inline stack
    /// nodes are no longer needed.
    DiscardInlined,
}

/// We use a compact representation of the new frames: (start index, len)
/// Examples:
///
/// - (5, 1) represents the sequence [5] (len 1)
/// - (3, 4) represents the sequence [3, 4, 5, 6] (len 4)
///
/// This only works if the new indexes are consecutive, which they should be.
/// Every frame should be new to new_frame_interner because it should be
/// a combination of (lib, address, inline depth) that the interner hasn't
/// seen before - it didn't contain any frames at all from lib before the
/// outer loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompactFrameSequence {
    first_frame_index: usize,
    frame_count: usize,
}

impl CompactFrameSequence {
    pub fn single_frame(frame_index: usize) -> Self {
        Self {
            first_frame_index: frame_index,
            frame_count: 1,
        }
    }

    /// Consume an iterator which yields frame indexes.
    ///
    /// The caller must guarantee the following:
    /// - The iterator yields at least one item.
    /// - The indexes are contiguous, i.e. each index is prev_index + 1.
    ///
    /// # Panics
    ///
    /// This function panics if the above guarantees are not upheld.
    pub fn from_iter(mut index_iter: impl Iterator<Item = usize>) -> Self {
        let first_frame_index = index_iter
            .next()
            .expect("Must have at least one index in CompactFrameSequence");
        let mut prev_index = first_frame_index;
        let mut frame_count = 1;
        for index in index_iter {
            assert_eq!(
                index,
                prev_index + 1,
                "Must have consecutive indexes in CompactFrameSequence."
            );
            prev_index = index;
            frame_count += 1;
        }
        Self {
            first_frame_index,
            frame_count,
        }
    }

    pub fn frame_index_iter(&self) -> std::ops::Range<usize> {
        self.first_frame_index..(self.first_frame_index + self.frame_count)
    }
}

/// Looks up the corresponding [`StringHandle`] for a [`SymbolStringIndex`],
/// inserting any new strings into the [`ProfileStringTable`] as needed.
pub struct StringTableAdapter<'a> {
    map: FastHashMap<SymbolStringIndex, StringHandle>,
    symbol_strings: &'a SymbolStringTable,
    string_table: &'a mut ProfileStringTable,
}

impl<'a> StringTableAdapter<'a> {
    pub fn new(
        symbol_strings: &'a SymbolStringTable,
        string_table: &'a mut ProfileStringTable,
    ) -> Self {
        Self {
            map: Default::default(),
            symbol_strings,
            string_table,
        }
    }

    pub fn convert_index(&mut self, symbol_string_index: SymbolStringIndex) -> StringHandle {
        *self.map.entry(symbol_string_index).or_insert_with(|| {
            self.string_table
                .index_for_string(self.symbol_strings.get_string(symbol_string_index))
        })
    }

    pub fn string_table_mut(&mut self) -> &mut ProfileStringTable {
        self.string_table
    }
}

/// Create a new [`FrameInterner`], [`NativeSymbols`] and [`StackTable`] from the existing
/// tables and the provided symbol information.
///
/// Also return an old_stack_to_new_stack Vec so that existing stack indexes can be updated
/// to refer to the new stack table.
pub fn apply_symbol_information(
    frame_interner: FrameInterner,
    native_symbols: NativeSymbols,
    stack_table: StackTable,
    libs: &FastHashSet<GlobalLibIndex>,
    lib_symbols: &BTreeMap<GlobalLibIndex, &LibSymbolInfo>,
    strings: &mut StringTableAdapter,
) -> (FrameInterner, NativeSymbols, StackTable, Vec<Option<usize>>) {
    let (mut new_native_symbols, old_native_symbol_to_new_native_symbol) =
        native_symbols.new_table_with_symbols_from_libs_removed(libs);

    let frames = frame_interner.into_frames();
    let mut new_frame_interner = FrameInterner::new();
    let (conversion_action_for_stack_frame, native_frames) = classify_frames(
        frames,
        &mut new_frame_interner,
        libs,
        &old_native_symbol_to_new_native_symbol,
    );

    let symbolicated_frames_by_key = create_symbolicated_frames(
        native_frames,
        lib_symbols,
        strings,
        &mut new_native_symbols,
        &mut new_frame_interner,
    );

    let (new_stack_table, old_stack_to_new_stack) = create_symbolicated_stacks(
        conversion_action_for_stack_frame,
        stack_table,
        symbolicated_frames_by_key,
    );

    (
        new_frame_interner,
        new_native_symbols,
        new_stack_table,
        old_stack_to_new_stack,
    )
}

/// Classify each frame in `frames` into a [`StackNodeConversionAction`] variant.
///
/// Add frames to `new_frame_interner` immediately if we don't need symbolication info for them.
/// Also return a set of all frames needing symbolication.
fn classify_frames(
    frames: impl Iterator<Item = InternalFrame>,
    new_frame_interner: &mut FrameInterner,
    libs: &FastHashSet<GlobalLibIndex>,
    old_native_symbol_to_new_native_symbol: &NativeSymbolIndexTranslator,
) -> (
    Vec<StackNodeConversionAction>,
    BTreeSet<FrameKeyForSymbolication>,
) {
    let mut native_frames = BTreeSet::new();
    let conversion_action_for_stack_frame = frames
        .map(|mut frame| match frame.variant {
            InternalFrameVariant::Label => {
                // Copy the frame into new_frame_interner as-is.
                let new_frame_index = new_frame_interner.index_for_frame(frame);
                StackNodeConversionAction::RemapIndex(new_frame_index)
            }
            InternalFrameVariant::Native(native) if libs.contains(&native.lib) => {
                if native.inline_depth == 0 {
                    let key = FrameKeyForSymbolication::new(&frame, &native);
                    native_frames.insert(key);
                    StackNodeConversionAction::Symbolicate(key)
                } else {
                    StackNodeConversionAction::DiscardInlined
                }
            }
            InternalFrameVariant::Native(mut native) => {
                // This is a native frame, but we're not applying any symbols for its
                // library, so just take it as-is and put it into new_frame_interner,
                // but with an updated native_symbol.
                if let Some(ns) = native.native_symbol {
                    native.native_symbol = Some(old_native_symbol_to_new_native_symbol.map(ns));
                    frame.variant = InternalFrameVariant::Native(native);
                }
                let new_frame_index = new_frame_interner.index_for_frame(frame);
                StackNodeConversionAction::RemapIndex(new_frame_index)
            }
        })
        .collect();

    (conversion_action_for_stack_frame, native_frames)
}

/// An iterator which matches a sequence of [`FrameKeyForSymbolication`] up with
/// the corresponding [`AddressInfo`] in a `BTreeMap<GlobalLibIndex, &LibSymbolInfo>`.
///
/// It takes advantage of the ordering to do a single pass over both containers, because the
/// information in both containers is ordered by (lib, address):
///
/// - The BTreeSet iterator yields frame keys based on the [`FrameKeyForSymbolication`]
///   order, which has the [`GlobalLibIndex`] as its first struct member and the address
///   within that library as its second struct member.
/// - The BTreeMap is keyed by [`GlobalLibIndex`], so it's ordered by lib.
/// - The [`LibSymbolInfo`] in the BTreeMap members stores its address info
///   by ascending address.
struct SymbolicationIter<'a> {
    native_frame_iter: btree_set::IntoIter<FrameKeyForSymbolication>,
    lib_symbols: &'a BTreeMap<GlobalLibIndex, &'a LibSymbolInfo>,
    current_lib: GlobalLibIndex,
    current_lib_symbols: &'a LibSymbolInfo,
    /// Index into `self.current_lib_symbols.sorted_addresses`, may point beyond the
    /// end of the array.
    current_lib_next_address_index: usize,
}

impl<'a> SymbolicationIter<'a> {
    /// Create the SymbolicationIter. Returns None if `native_frames` is empty.
    ///
    /// Panics if `native_frames` contains any frames with a [`GlobalLibIndex`]
    /// that's not present in `lib_symbols`.
    pub fn new(
        native_frames: BTreeSet<FrameKeyForSymbolication>,
        lib_symbols: &'a BTreeMap<GlobalLibIndex, &LibSymbolInfo>,
    ) -> Option<Self> {
        let first_frame = native_frames.iter().next()?;
        let current_lib = first_frame.lib;
        let current_lib_symbols = lib_symbols.get(&current_lib).unwrap();
        let current_lib_next_address_index = 0;
        let native_frame_iter = native_frames.into_iter();
        Some(Self {
            native_frame_iter,
            lib_symbols,
            current_lib,
            current_lib_symbols,
            current_lib_next_address_index,
        })
    }

    fn advance_to_lib(&mut self, lib: GlobalLibIndex) {
        self.current_lib = lib;
        self.current_lib_symbols = self.lib_symbols.get(&lib).unwrap();
        self.current_lib_next_address_index = 0;
    }

    /// Within `self.current_lib_symbols`, advance the current index into
    /// `self.current_lib_symbols.sorted_addresses` so that it points at an
    /// address which is >= `address`.
    /// If it now points at an address which is == `address`, return the corresponding
    /// AddressInfo, otherwise return None.
    fn advance_to_address(&mut self, address: u32) -> Option<&'a AddressInfo> {
        let mut i = self.current_lib_next_address_index;
        let sorted_addresses = &self.current_lib_symbols.sorted_addresses;

        while i < sorted_addresses.len() && sorted_addresses[i] < address {
            i += 1;
        }

        self.current_lib_next_address_index = i;
        if i < sorted_addresses.len() && sorted_addresses[i] == address {
            Some(&self.current_lib_symbols.address_infos[i])
        } else {
            None
        }
    }
}

impl<'a> Iterator for SymbolicationIter<'a> {
    type Item = (FrameKeyForSymbolication, Option<&'a AddressInfo>);

    fn next(&mut self) -> Option<Self::Item> {
        let frame_key = self.native_frame_iter.next()?;

        if frame_key.lib != self.current_lib {
            self.advance_to_lib(frame_key.lib);
        }

        let address_info = self.advance_to_address(frame_key.address);

        Some((frame_key, address_info))
    }
}

/// Create the native symbols and frames for all old frames in `native_frames`,
/// using the symbol information from `lib_symbols` and putting the new objects
/// into `new_native_symbols` and `new_frame_interner`.
///
/// The caller guarantees that `new_frame_interner` does not currently contain
/// any frames for any of the libs in `lib_symbols`.
fn create_symbolicated_frames(
    native_frames: BTreeSet<FrameKeyForSymbolication>,
    lib_symbols: &BTreeMap<GlobalLibIndex, &LibSymbolInfo>,
    strings: &mut StringTableAdapter,
    new_native_symbols: &mut NativeSymbols,
    new_frame_interner: &mut FrameInterner,
) -> BTreeMap<FrameKeyForSymbolication, CompactFrameSequence> {
    let Some(symbolication_iter) = SymbolicationIter::new(native_frames, lib_symbols) else {
        return BTreeMap::new();
    };

    symbolication_iter
        .map(|(frame_key, address_info)| {
            let new_frames = create_symbolicated_frames_for_frame(
                frame_key,
                address_info,
                strings,
                new_native_symbols,
                new_frame_interner,
            );
            (frame_key, new_frames)
        })
        .collect()
}

/// Create one or more (if there's inlining) new frames for the supplied frame_key,
/// and return their indexes as a [`CompactFrameSequence`].
fn create_symbolicated_frames_for_frame(
    frame_key: FrameKeyForSymbolication,
    address_info: Option<&AddressInfo>,
    strings: &mut StringTableAdapter,
    new_native_symbols: &mut NativeSymbols,
    new_frame_interner: &mut FrameInterner,
) -> CompactFrameSequence {
    let FrameKeyForSymbolication {
        lib,
        address,
        subcategory,
        frame_flags,
    } = frame_key;

    let Some(address_info) = address_info else {
        // There was no AddressInfo for this address in the supplied ProfileLibSymbols.
        // Create a "0xa63cf4" frame with the raw address, and no native symbol.
        let name = strings
            .string_table_mut()
            .index_for_hex_address_string(address.into());
        let new_frame = new_frame_interner.index_for_frame(InternalFrame {
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
        return CompactFrameSequence::single_frame(new_frame);
    };

    // We have symbol information for this address!
    let AddressInfo {
        symbol_name,
        symbol_start_address,
        symbol_size,
        ref frames,
    } = *address_info;

    // First, create a NativeSymbol for the outer function, describing the
    // address range of the machine code (for the assembly view), and the linkage name.
    let symbol_name_string_index = strings.convert_index(symbol_name);
    let native_symbol_index = new_native_symbols.symbol_index_for_symbol(
        lib,
        symbol_start_address,
        symbol_size,
        symbol_name_string_index,
    );

    // Now create the frames + functions.

    if frames.is_empty() {
        // address_info.frames is empty, which means we don't have file + line info
        // or inlined frames.
        // We create a single frame for the outer function, and its function name
        // is the same as the symbol name.
        let new_frame = new_frame_interner.index_for_frame(InternalFrame {
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
        return CompactFrameSequence::single_frame(new_frame);
    }

    // We have function name + file path + line info for this address, and it
    // contains at least one frame.
    // We may even have multiple frames, if there's inlining.
    // Create one InternalFrame for each of the frames, and store their new
    // indexes in new_frame_indexes.
    //
    // If we have inlined frames, `frames` has the innermost frame first.
    // Use .rev() so that this loop sees the outer function first and the
    // innermost frame last.
    let frame_iter = frames.iter().rev();

    // We use a compact representation of the new frames, see CompactFrameSequence.
    // This only works if the new indexes are consecutive, which they should be.
    // Every frame should be new to new_frame_interner because it should be
    // a combination of (lib, address, inline depth) that the interner hasn't
    // seen before - it didn't contain any frames at all from lib before
    // `create_symbolicated_frames` was called.
    CompactFrameSequence::from_iter(frame_iter.enumerate().map(|(inline_depth, frame)| {
        new_frame_interner.index_for_frame(InternalFrame {
            name: strings.convert_index(frame.function_name),
            variant: InternalFrameVariant::Native(NativeFrameData {
                lib,
                native_symbol: Some(native_symbol_index),
                relative_address: address,
                inline_depth: inline_depth as u16,
            }),
            subcategory,
            source_location: SourceLocation {
                file_path: frame.file.map(|f| strings.convert_index(f)),
                line: frame.line,
                col: None,
            },
            flags: frame_flags,
        })
    }))
}

/// From the old stack table, create a new stack table referring to the new
/// frames, and create an old_stack_to_new_stack conversion map.
///
/// Go through the stack table. For each stack node:
/// Translate prefix using old_stack_to_new_stack.
/// Then check conversion_action_for_stack_frame[old_frame_index]:
/// - RemapIndex:
///   - create stack node with new indexes
///   - old_stack_to_new_stack[old_stack] = new_stack
/// - Symbolicate:
///   - look up (outer_frame_index, frame_count) in symbolicated_frames_by_lib_and_address
///   - create frame_count new stack nodes. outer node has translated prefix,
///     each next node has the just-created stack as its parent
///   - old_stack_to_new_stack[old_stack] = new_stack for innermost frame
/// - DiscardInlined:
///   - old_stack_to_new_stack[old_stack] = translated prefix
fn create_symbolicated_stacks(
    conversion_action_for_stack_frame: Vec<StackNodeConversionAction>,
    stack_table: StackTable,
    symbolicated_frames_by_key: BTreeMap<FrameKeyForSymbolication, CompactFrameSequence>,
) -> (StackTable, Vec<Option<usize>>) {
    let mut new_stack_table = StackTable::new();
    let mut old_stack_to_new_stack = Vec::with_capacity(stack_table.len());

    for (old_prefix, old_frame) in stack_table.into_stacks() {
        let new_prefix = old_prefix.and_then(|old_prefix| old_stack_to_new_stack[old_prefix]);
        let new_stack = match &conversion_action_for_stack_frame[old_frame] {
            StackNodeConversionAction::RemapIndex(new_frame) => {
                Some(new_stack_table.index_for_stack(new_prefix, *new_frame))
            }
            StackNodeConversionAction::DiscardInlined => new_prefix,
            StackNodeConversionAction::Symbolicate(frame_key_for_symbolication) => {
                let expanded_frames = symbolicated_frames_by_key[frame_key_for_symbolication];
                let mut current_stack = new_prefix;
                for frame_index in expanded_frames.frame_index_iter() {
                    current_stack =
                        Some(new_stack_table.index_for_stack(current_stack, frame_index));
                }
                current_stack
            }
        };
        old_stack_to_new_stack.push(new_stack);
    }

    (new_stack_table, old_stack_to_new_stack)
}
