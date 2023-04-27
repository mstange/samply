use std::cmp::Reverse;
use std::collections::BinaryHeap;

use fxprof_processed_profile::{ProcessHandle, ThreadHandle};

use crate::shared::{jit_function_recycler::JitFunctionRecycler, types::FastHashMap};

pub struct ProcessRecyclingData {
    pub process_handle: ProcessHandle,
    pub main_thread_handle: ThreadHandle,
    pub thread_recycler: ThreadRecycler,
    pub jit_function_recycler: JitFunctionRecycler,
}

impl PartialEq for ProcessRecyclingData {
    fn eq(&self, other: &Self) -> bool {
        self.process_handle == other.process_handle
    }
}

impl Eq for ProcessRecyclingData {}

impl PartialOrd for ProcessRecyclingData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.process_handle.partial_cmp(&other.process_handle)
    }
}

impl Ord for ProcessRecyclingData {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.process_handle.cmp(&other.process_handle)
    }
}

pub type ProcessRecycler = RecyclerByName<ProcessRecyclingData>;
pub type ThreadRecycler = RecyclerByName<ThreadHandle>;

pub struct RecyclerByName<T: Ord>(FastHashMap<String, BinaryHeap<Reverse<T>>>);

impl<T: Ord> RecyclerByName<T> {
    pub fn new() -> Self {
        Self(FastHashMap::default())
    }

    pub fn add_to_pool(&mut self, name: &str, value: T) {
        self.0
            .entry(name.to_string())
            .or_default()
            .push(Reverse(value));
    }

    pub fn recycle_by_name(&mut self, name: &str) -> Option<T> {
        let heap = self.0.get_mut(name)?;
        let process: Reverse<T> = heap
            .pop()
            .expect("We only have non-empty BinaryHeaps in this HashMap");
        if heap.is_empty() {
            self.0.remove(name);
        }
        Some(process.0)
    }
}
