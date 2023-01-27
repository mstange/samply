use std::cmp::Ordering;

use crate::fast_hash_map::FastHashMap;
use crate::frame_table::InternalFrameLocation;
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::lib_info::Lib;
use crate::lib_ranges::{LibRange, LibRanges};
use crate::library_info::LibraryInfo;
use crate::Timestamp;

/// A thread. Can be created with [`Profile::add_thread`](crate::Profile::add_thread).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ThreadHandle(pub(crate) usize);

/// The index of a library within a process.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ProcessLibIndex(usize);

#[derive(Debug)]
pub struct Process {
    pid: u32,
    name: String,
    threads: Vec<ThreadHandle>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    libs: Vec<Lib>,
    lib_ranges: LibRanges<ProcessLibIndex>,
    used_lib_map: FastHashMap<ProcessLibIndex, GlobalLibIndex>,
}

impl Process {
    pub fn new(name: &str, pid: u32, start_time: Timestamp) -> Self {
        Self {
            pid,
            threads: Vec::new(),
            lib_ranges: LibRanges::new(),
            used_lib_map: FastHashMap::default(),
            libs: Vec::new(),
            start_time,
            end_time: None,
            name: name.to_owned(),
        }
    }

    pub fn set_start_time(&mut self, start_time: Timestamp) {
        self.start_time = start_time;
    }

    pub fn start_time(&self) -> Timestamp {
        self.start_time
    }

    pub fn set_end_time(&mut self, end_time: Timestamp) {
        self.end_time = Some(end_time);
    }

    pub fn end_time(&self) -> Option<Timestamp> {
        self.end_time
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn add_thread(&mut self, thread: ThreadHandle) {
        self.threads.push(thread);
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn cmp_for_json_order(&self, other: &Process) -> Ordering {
        if let Some(ordering) = self.start_time.partial_cmp(&other.start_time) {
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.pid.cmp(&other.pid)
    }

    pub fn threads(&self) -> Vec<ThreadHandle> {
        self.threads.clone()
    }

    pub fn convert_address(
        &mut self,
        global_libs: &mut GlobalLibTable,
        address: u64,
    ) -> InternalFrameLocation {
        let range = match self.lib_ranges.lookup(address) {
            Some(range) => range,
            None => return InternalFrameLocation::UnknownAddress(address),
        };
        let process_lib = range.lib_index;
        let relative_address = (address - range.base) as u32;
        let lib_index = self.convert_lib_index(process_lib, global_libs);
        InternalFrameLocation::AddressInLib(relative_address, lib_index)
    }

    pub fn convert_lib_index(
        &mut self,
        process_lib: ProcessLibIndex,
        global_libs: &mut GlobalLibTable,
    ) -> GlobalLibIndex {
        let libs = &self.libs;
        *self
            .used_lib_map
            .entry(process_lib)
            .or_insert_with(|| global_libs.index_for_lib(libs[process_lib.0].clone()))
    }

    pub fn add_lib(&mut self, lib: LibraryInfo) {
        let lib_index = ProcessLibIndex(self.libs.len());
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
}
