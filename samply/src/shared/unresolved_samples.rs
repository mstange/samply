use std::collections::hash_map::Entry;
use std::collections::BTreeMap;

use fxprof_processed_profile::{CpuDelta, FrameHandle, MarkerHandle, ThreadHandle, Timestamp};
use rustc_hash::FxBuildHasher;
use schnellru::{ByLength, LruMap};

use super::context_switch::OffCpuSampleGroup;
use super::timestamp_converter::TimestampConverter;
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
        extra_label_frame: Option<FrameHandle>,
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
        extra_label_frame: Option<FrameHandle>,
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

    /// Expand an [`OffCpuSampleGroup`] into the samples that represent a paused
    /// (off-cpu) range on a thread, all sharing the same stack:
    ///  - a "first sample" at the start of the range, carrying any leftover
    ///    accumulated running time (`cpu_delta`), and
    ///  - if the range spans more than one sampling interval, a zero-cpu "rest
    ///    sample" at the end covering the remainder.
    ///
    /// `stack_timestamp_mono` is the raw time at which `off_cpu_stack` was
    /// *captured*, which is not the same as where either sample is placed: both
    /// samples show the one stack, so both must resolve their addresses against
    /// the library mappings as of that one capture time. The platforms differ on
    /// when that is — Linux saves the stack at switch-out (the start of the
    /// range), Windows gets it at switch-in (the end) — which is why the caller
    /// passes it rather than us picking begin or end here.
    #[allow(clippy::too_many_arguments)]
    pub fn add_off_cpu_sample_group(
        &mut self,
        off_cpu_sample: OffCpuSampleGroup,
        thread_handle: ThreadHandle,
        cpu_delta: CpuDelta,
        timestamp_converter: &TimestampConverter,
        off_cpu_weight_per_sample: i32,
        off_cpu_stack: UnresolvedStackHandle,
        stack_timestamp_mono: u64,
    ) {
        let OffCpuSampleGroup {
            begin_timestamp: begin_timestamp_raw,
            end_timestamp: end_timestamp_raw,
            sample_count,
        } = off_cpu_sample;

        let begin_timestamp = timestamp_converter.convert_time(begin_timestamp_raw);
        self.add_sample(
            thread_handle,
            begin_timestamp,
            stack_timestamp_mono,
            off_cpu_stack,
            cpu_delta,
            off_cpu_weight_per_sample,
            None,
        );

        if sample_count > 1 {
            let weight = i32::try_from(sample_count - 1).unwrap_or(0) * off_cpu_weight_per_sample;
            let end_timestamp = timestamp_converter.convert_time(end_timestamp_raw);
            self.add_sample(
                thread_handle,
                end_timestamp,
                stack_timestamp_mono,
                off_cpu_stack,
                CpuDelta::ZERO,
                weight,
                None,
            );
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
    pub extra_label_frame: Option<FrameHandle>,
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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnresolvedStackHandle(u32);

impl UnresolvedStackHandle {
    /// Represents the empty stack / the root stack node
    pub const EMPTY: Self = Self(u32::MAX);
}

/// An interner that recursively interns stacks in the form of (prefix, frame).
///
/// The interner avoids the worst case O(n) runtime behavior of hash maps due to rehashes. Under the
/// hood, BTreeMap is used to store the full mapping with worst case O(log n) inserts, while an LRU
/// hash map maintains a O(1) fast path for the majority of accesses.
pub struct UnresolvedStacks {
    pub stacks: Vec<(UnresolvedStackHandle, StackFrame)>, // (prefix, frame)
    stack_lookup: BTreeMap<(UnresolvedStackHandle, StackFrame), UnresolvedStackHandle>, // (prefix, frame) -> stack index
    stack_cache:
        LruMap<(UnresolvedStackHandle, StackFrame), UnresolvedStackHandle, ByLength, FxBuildHasher>,
}

impl UnresolvedStacks {
    // Two things to consider for the size of table:
    // - The cache should be small enough to fit in CPU cache
    // - LruMap uses SwissTable under the hood where erases use tombstones and rehashing is required
    //   for compaction. The cache should be small enough to bound rehashing cost
    //
    // Assuming 64B per entry, a 256KB L2 can fit 4K entries, and rehashing time seems to be under 1ms.
    const CACHE_SIZE: u32 = 4096;
}

impl Default for UnresolvedStacks {
    fn default() -> Self {
        Self {
            stacks: Vec::new(),
            stack_cache: LruMap::with_hasher(ByLength::new(Self::CACHE_SIZE), FxBuildHasher),
            stack_lookup: BTreeMap::new(),
        }
    }
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
            if let Some(node) = self.stack_cache.get(&x).copied() {
                prefix = node;
                continue;
            }

            let node = *self.stack_lookup.entry(x).or_insert_with(|| {
                let new_index = self.stacks.len() as u32;
                self.stacks.push(x);
                UnresolvedStackHandle(new_index)
            });
            self.stack_cache.insert(x, node);
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
        self.convert_with_prefix(
            UnresolvedStackHandle::EMPTY,
            frames.filter(|f| f.stack_mode() != Some(StackMode::Kernel)),
        )
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
