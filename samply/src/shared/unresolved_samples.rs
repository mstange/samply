use std::collections::hash_map::Entry;

use fxprof_processed_profile::{CpuDelta, FrameInfo, MarkerHandle, ThreadHandle, Timestamp};

use super::types::{FastHashMap, StackFrame, StackMode};

#[derive(Debug, Clone, Default)]
pub struct UnresolvedSamples {
    samples_and_markers: Vec<UnresolvedSampleOrMarker>,
    prev_sample_info_per_thread: FastHashMap<ThreadHandle, PreviousSampleInfo>,
}

#[derive(Debug, Clone)]
struct PreviousSampleInfo {
    stack: UnresolvedStackHandle,
    prev_sample_index_if_zero_cpu: Option<usize>,
}

impl UnresolvedSamples {
    pub fn into_inner(self) -> Vec<UnresolvedSampleOrMarker> {
        self.samples_and_markers
    }

    pub fn is_empty(&self) -> bool {
        self.samples_and_markers.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_sample(
        &mut self,
        thread_handle: ThreadHandle,
        timestamp: Timestamp,
        timestamp_mono: u64,
        stack: UnresolvedStackHandle,
        cpu_delta: CpuDelta,
        weight: i32,
        extra_label_frame: Option<FrameInfo>,
    ) {
        let sample_index = self.samples_and_markers.len();
        self.samples_and_markers.push(UnresolvedSampleOrMarker {
            thread_handle,
            timestamp,
            timestamp_mono,
            stack,
            extra_label_frame,
            sample_or_marker: SampleOrMarker::Sample(SampleData { weight, cpu_delta }),
        });
        self.prev_sample_info_per_thread.insert(
            thread_handle,
            PreviousSampleInfo {
                stack,
                prev_sample_index_if_zero_cpu: (cpu_delta == CpuDelta::ZERO)
                    .then_some(sample_index),
            },
        );
    }

    #[allow(unused)]
    pub fn add_sample_same_stack_zero_cpu(
        &mut self,
        thread_handle: ThreadHandle,
        timestamp: Timestamp,
        timestamp_mono: u64,
        weight: i32,
        extra_label_frame: Option<FrameInfo>,
    ) {
        match self.prev_sample_info_per_thread.entry(thread_handle) {
            Entry::Occupied(mut entry) => {
                let sample_info = entry.get_mut();
                if let Some(sample_index) = sample_info.prev_sample_index_if_zero_cpu {
                    let sample = &mut self.samples_and_markers[sample_index];
                    sample.timestamp = timestamp;
                    let SampleOrMarker::Sample(ref mut data) = &mut sample.sample_or_marker else {
                        panic!()
                    };
                    data.weight += weight;
                } else {
                    let stack = sample_info.stack;
                    let sample_index = self.samples_and_markers.len();
                    self.samples_and_markers.push(UnresolvedSampleOrMarker {
                        thread_handle,
                        timestamp,
                        timestamp_mono,
                        stack,
                        extra_label_frame,
                        sample_or_marker: SampleOrMarker::Sample(SampleData {
                            weight,
                            cpu_delta: CpuDelta::ZERO,
                        }),
                    });
                    sample_info.prev_sample_index_if_zero_cpu = Some(sample_index);
                }
            }
            Entry::Vacant(entry) => {
                let stack = UnresolvedStackHandle::EMPTY;
                let sample_index = self.samples_and_markers.len();
                self.samples_and_markers.push(UnresolvedSampleOrMarker {
                    thread_handle,
                    timestamp,
                    timestamp_mono,
                    stack,
                    extra_label_frame,
                    sample_or_marker: SampleOrMarker::Sample(SampleData {
                        weight,
                        cpu_delta: CpuDelta::ZERO,
                    }),
                });
                entry.insert(PreviousSampleInfo {
                    stack,
                    prev_sample_index_if_zero_cpu: Some(sample_index),
                });
            }
        }
    }

    pub fn attach_stack_to_marker(
        &mut self,
        thread_handle: ThreadHandle,
        timestamp: Timestamp,
        timestamp_mono: u64,
        stack: UnresolvedStackHandle,
        marker_handle: MarkerHandle,
    ) {
        self.samples_and_markers.push(UnresolvedSampleOrMarker {
            thread_handle,
            timestamp,
            timestamp_mono,
            stack,
            extra_label_frame: None,
            sample_or_marker: SampleOrMarker::MarkerHandle(marker_handle),
        });
    }
}

#[derive(Debug, Clone)]
pub struct UnresolvedSampleOrMarker {
    pub thread_handle: ThreadHandle,
    pub timestamp: Timestamp,
    pub timestamp_mono: u64,
    pub stack: UnresolvedStackHandle,
    pub extra_label_frame: Option<FrameInfo>,
    pub sample_or_marker: SampleOrMarker,
}

#[derive(Debug, Clone)]
pub enum SampleOrMarker {
    Sample(SampleData),
    MarkerHandle(MarkerHandle),
}

#[derive(Debug, Clone)]
pub struct SampleData {
    pub cpu_delta: CpuDelta,
    pub weight: i32,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct UnresolvedStackHandle(u32);

impl UnresolvedStackHandle {
    /// Represents the empty stack / the root stack node
    pub const EMPTY: Self = Self(u32::MAX);
}

#[derive(Debug, Clone, Default)]
pub struct UnresolvedStacks {
    pub stacks: Vec<(UnresolvedStackHandle, StackFrame)>, // (prefix, frame)
    pub stack_lookup: FastHashMap<(UnresolvedStackHandle, StackFrame), UnresolvedStackHandle>, // (prefix, frame) -> stack index
}

impl UnresolvedStacks {
    /// Get the `UnresolvedStackHandle` for a stack. The stack must be ordered from
    /// caller-most to callee-most ("outside to inside").
    pub fn convert(&mut self, frames: impl Iterator<Item = StackFrame>) -> UnresolvedStackHandle {
        self.convert_with_prefix(UnresolvedStackHandle::EMPTY, frames)
    }

    pub fn convert_with_prefix(
        &mut self,
        mut prefix: UnresolvedStackHandle,
        frames: impl Iterator<Item = StackFrame>,
    ) -> UnresolvedStackHandle {
        for frame in frames {
            let x = (prefix, frame);
            let node = *self.stack_lookup.entry(x).or_insert_with(|| {
                let new_index = self.stacks.len() as u32;
                self.stacks.push(x);
                UnresolvedStackHandle(new_index)
            });
            prefix = node;
        }
        prefix
    }

    /// Get the `UnresolvedStackHandle` for a stack, skipping any kernel frames.
    /// The stack must be ordered from caller-most to callee-most ("outside to inside").
    pub fn convert_no_kernel(
        &mut self,
        frames: impl Iterator<Item = StackFrame>,
    ) -> UnresolvedStackHandle {
        let mut prefix = UnresolvedStackHandle::EMPTY;
        for frame in frames.filter(|f| f.stack_mode() != Some(StackMode::Kernel)) {
            let x = (prefix, frame);
            let node = *self.stack_lookup.entry(x).or_insert_with(|| {
                let new_index = self.stacks.len() as u32;
                self.stacks.push(x);
                UnresolvedStackHandle(new_index)
            });
            prefix = node;
        }
        prefix
    }

    // Appends the stack to `buf`, starting with the callee-most frame.
    pub fn convert_back(&self, mut stack_index: UnresolvedStackHandle, buf: &mut Vec<StackFrame>) {
        while stack_index != UnresolvedStackHandle::EMPTY {
            let (prefix, frame) = self.stacks[stack_index.0 as usize];
            buf.push(frame);
            stack_index = prefix;
        }
    }
}
