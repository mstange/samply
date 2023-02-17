use std::cmp::Ordering;
use std::hash::Hash;

use crate::frame_table::InternalFrameLocation;
use crate::global_lib_table::GlobalLibTable;
use crate::library_info::LibraryInfo;
use crate::libs_with_ranges::LibsWithRanges;
use crate::Timestamp;

/// A thread. Can be created with [`Profile::add_thread`](crate::Profile::add_thread).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ThreadHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Process {
    pid: String,
    name: String,
    threads: Vec<ThreadHandle>,
    start_time: Timestamp,
    end_time: Option<Timestamp>,
    libs: LibsWithRanges,
}

impl Process {
    pub fn new(name: &str, pid: String, start_time: Timestamp) -> Self {
        Self {
            pid,
            threads: Vec::new(),
            libs: LibsWithRanges::new(),
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

    pub fn pid(&self) -> &str {
        &self.pid
    }

    pub fn cmp_for_json_order(&self, other: &Process) -> Ordering {
        if let Some(ordering) = self.start_time.partial_cmp(&other.start_time) {
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.pid.cmp(&other.pid)
    }

    pub fn threads(&self) -> &[ThreadHandle] {
        &self.threads
    }

    pub fn convert_address(
        &mut self,
        global_libs: &mut GlobalLibTable,
        kernel_libs: &mut LibsWithRanges,
        address: u64,
    ) -> InternalFrameLocation {
        // Try to find the address in the kernel libs first, and then in the process libs.
        match kernel_libs
            .convert_address(global_libs, address)
            .or_else(|| self.libs.convert_address(global_libs, address))
        {
            Some((relative_address, global_lib_index)) => {
                InternalFrameLocation::AddressInLib(relative_address, global_lib_index)
            }
            None => InternalFrameLocation::UnknownAddress(address),
        }
    }

    pub fn add_lib(&mut self, lib: LibraryInfo) {
        self.libs.add_lib(lib);
    }

    pub fn unload_lib(&mut self, base_address: u64) {
        self.libs.unload_lib(base_address);
    }
}
