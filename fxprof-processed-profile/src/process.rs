use std::cmp::Ordering;
use std::hash::Hash;

use crate::frame_table::InternalFrameLocation;
use crate::global_lib_table::{GlobalLibTable, LibraryHandle};
use crate::lib_mappings::LibMappings;
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
    libs: LibMappings<LibraryHandle>,
}

impl Process {
    pub fn new(name: &str, pid: String, start_time: Timestamp) -> Self {
        Self {
            pid,
            threads: Vec::new(),
            libs: LibMappings::new(),
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
        kernel_libs: &mut LibMappings<LibraryHandle>,
        address: u64,
    ) -> InternalFrameLocation {
        // Try to find the address in the kernel libs first, and then in the process libs.
        match kernel_libs
            .convert_address(address)
            .or_else(|| self.libs.convert_address(address))
        {
            Some((relative_address, lib_handle)) => {
                let global_lib_index = global_libs.index_for_used_lib(*lib_handle);
                InternalFrameLocation::AddressInLib(relative_address, global_lib_index)
            }
            None => InternalFrameLocation::UnknownAddress(address),
        }
    }

    pub fn add_lib_mapping(
        &mut self,
        lib: LibraryHandle,
        start_avma: u64,
        end_avma: u64,
        relative_address_at_start: u32,
    ) {
        self.libs
            .add_mapping(start_avma, end_avma, relative_address_at_start, lib);
    }

    pub fn remove_lib_mapping(&mut self, start_avma: u64) {
        self.libs.remove_mapping(start_avma);
    }

    pub fn remove_all_lib_mappings(&mut self) {
        self.libs.clear();
    }
}
