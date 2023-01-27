use std::hash::Hash;

use crate::fast_hash_map::FastHashMap;
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::lib_info::Lib;
use crate::LibraryInfo;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct InternalLibIndex(usize);

#[derive(Debug, Clone)]
pub struct LibsWithRanges {
    libs: Vec<Lib>,
    lib_ranges: LibRanges<InternalLibIndex>,
    used_libs: FastHashMap<InternalLibIndex, GlobalLibIndex>,
}

impl LibsWithRanges {
    pub fn new() -> Self {
        Self {
            libs: Vec::new(),
            lib_ranges: LibRanges::new(),
            used_libs: FastHashMap::default(),
        }
    }

    pub fn add_lib(&mut self, lib: LibraryInfo) {
        let lib_index = InternalLibIndex(self.libs.len());
        self.libs.push(Lib {
            name: lib.name,
            debug_name: lib.debug_name,
            path: lib.path,
            debug_path: lib.debug_path,
            arch: lib.arch,
            debug_id: lib.debug_id,
            code_id: lib.code_id,
            symbol_table: lib.symbol_table,
        });

        self.lib_ranges.insert(LibRange {
            lib_index,
            base: lib.base_avma,
            start: lib.avma_range.start,
            end: lib.avma_range.end,
        });
    }

    pub fn unload_lib(&mut self, base_address: u64) {
        self.lib_ranges.remove(base_address);
    }

    pub fn convert_address(
        &mut self,
        global_libs: &mut GlobalLibTable,
        address: u64,
    ) -> Option<(u32, GlobalLibIndex)> {
        let range = match self.lib_ranges.lookup(address) {
            Some(range) => range,
            None => return None,
        };
        let relative_address = (address - range.base) as u32;
        let lib = &self.libs[range.lib_index.0];
        let global_lib_index = *self
            .used_libs
            .entry(range.lib_index)
            .or_insert_with(|| global_libs.index_for_lib(lib.clone()));
        Some((relative_address, global_lib_index))
    }
}

#[derive(Debug, Clone)]
struct LibRanges<I> {
    sorted_lib_ranges: Vec<LibRange<I>>,
}

impl<I> Default for LibRanges<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> LibRanges<I> {
    pub fn new() -> Self {
        Self {
            sorted_lib_ranges: Vec::new(),
        }
    }

    pub fn insert(&mut self, range: LibRange<I>) {
        let insertion_index = match self
            .sorted_lib_ranges
            .binary_search_by_key(&range.start, |r| r.start)
        {
            Ok(i) => {
                // We already have a library mapping at this address.
                // Not sure how to best deal with it. Ideally it wouldn't happen. Let's just remove this mapping.
                self.sorted_lib_ranges.remove(i);
                i
            }
            Err(i) => i,
        };

        self.sorted_lib_ranges.insert(insertion_index, range);
    }

    pub fn remove(&mut self, base_address: u64) {
        self.sorted_lib_ranges.retain(|r| r.base != base_address);
    }

    pub fn lookup(&self, address: u64) -> Option<&LibRange<I>> {
        let ranges = &self.sorted_lib_ranges[..];
        let index = match ranges.binary_search_by_key(&address, |r| r.start) {
            Err(0) => return None,
            Ok(exact_match) => exact_match,
            Err(insertion_index) => {
                let range_index = insertion_index - 1;
                if address < ranges[range_index].end {
                    range_index
                } else {
                    return None;
                }
            }
        };
        Some(&ranges[index])
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
struct LibRange<I> {
    start: u64,
    end: u64,
    lib_index: I,
    base: u64,
}
