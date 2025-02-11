use std::fmt::{Display, Formatter};

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::cpu_delta::CpuDelta;
use crate::serialization_helpers::{SerializableSingleValueColumn, SliceWithPermutation};
use crate::timestamp::{
    SerializableTimestampSliceAsDeltas, SerializableTimestampSliceAsDeltasWithPermutation,
    Timestamp,
};

/// The sample table contains stacks with timestamps and some extra information.
///
/// In the most common case, this is used for time-based sampling: At a fixed but
/// configurable rate, a profiler samples the current stack of each thread and records
/// it in the profile.
#[derive(Debug, Clone)]
pub struct SampleTable {
    sample_weight_type: WeightType,
    sample_weights: Vec<i32>,
    sample_timestamps: Vec<Timestamp>,
    /// An index into the thread's stack table for each sample. `None` means the empty stack.
    sample_stack_indexes: Vec<Option<usize>>,
    /// CPU usage delta since the previous sample for this thread, for each sample.
    sample_cpu_deltas: Vec<CpuDelta>,
    is_sorted_by_time: bool,
    last_sample_timestamp: Timestamp,
}

/// Specifies the meaning of the "weight" value of a thread's samples.
#[derive(Debug, Clone)]
pub enum WeightType {
    /// The weight is an integer multiplier. For example, "this stack was
    /// observed n times when sampling at the specified interval."
    ///
    /// This affects the total + self score of each call node in the call tree,
    /// and the order in the tree because the tree is ordered from large "totals"
    /// to small "totals".
    /// It also affects the width of the sample's stack's box in the flame graph.
    Samples,
    /// The weight is a duration in (fractional) milliseconds.
    ///
    /// Note that, since [`Profile::add_sample`](crate::Profile::add_sample) currently
    /// only accepts integer weight values, the usefulness of `TracingMs` is
    /// currently limited.
    TracingMs,
    /// The weight of each sample is a value in bytes.
    ///
    /// This can be used for profiles with allocation stacks. It can also be used
    /// for "size" profiles which give a bytes breakdown of the contents of a file.
    Bytes,
}

impl Display for WeightType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WeightType::Samples => write!(f, "samples"),
            WeightType::TracingMs => write!(f, "tracing-ms"),
            WeightType::Bytes => write!(f, "bytes"),
        }
    }
}

impl Serialize for WeightType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            WeightType::Samples => serializer.serialize_str("samples"),
            WeightType::TracingMs => serializer.serialize_str("tracing-ms"),
            WeightType::Bytes => serializer.serialize_str("bytes"),
        }
    }
}

impl SampleTable {
    pub fn new() -> Self {
        Self {
            sample_weight_type: WeightType::Samples,
            sample_weights: Vec::new(),
            sample_timestamps: Vec::new(),
            sample_stack_indexes: Vec::new(),
            sample_cpu_deltas: Vec::new(),
            is_sorted_by_time: true,
            last_sample_timestamp: Timestamp::from_nanos_since_reference(0),
        }
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        self.sample_weights.push(weight);
        self.sample_timestamps.push(timestamp);
        self.sample_stack_indexes.push(stack_index);
        self.sample_cpu_deltas.push(cpu_delta);
        if timestamp < self.last_sample_timestamp {
            self.is_sorted_by_time = false;
        }
        self.last_sample_timestamp = timestamp;
    }

    pub fn set_weight_type(&mut self, t: WeightType) {
        self.sample_weight_type = t;
    }

    pub fn modify_last_sample(&mut self, timestamp: Timestamp, weight: i32) {
        *self.sample_weights.last_mut().unwrap() += weight;
        *self.sample_timestamps.last_mut().unwrap() = timestamp;
    }
}

impl Serialize for SampleTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.sample_timestamps.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("weightType", &self.sample_weight_type.to_string())?;

        if self.is_sorted_by_time {
            map.serialize_entry("stack", &self.sample_stack_indexes)?;
            map.serialize_entry(
                "timeDeltas",
                &SerializableTimestampSliceAsDeltas(&self.sample_timestamps),
            )?;
            map.serialize_entry("weight", &self.sample_weights)?;
            map.serialize_entry("threadCPUDelta", &self.sample_cpu_deltas)?;
        } else {
            let mut indexes: Vec<usize> = (0..self.sample_timestamps.len()).collect();
            indexes.sort_unstable_by_key(|index| self.sample_timestamps[*index]);
            map.serialize_entry(
                "stack",
                &SliceWithPermutation(&self.sample_stack_indexes, &indexes),
            )?;
            map.serialize_entry(
                "timeDeltas",
                &SerializableTimestampSliceAsDeltasWithPermutation(
                    &self.sample_timestamps,
                    &indexes,
                ),
            )?;
            map.serialize_entry(
                "weight",
                &SliceWithPermutation(&self.sample_weights, &indexes),
            )?;
            map.serialize_entry(
                "threadCPUDelta",
                &SliceWithPermutation(&self.sample_cpu_deltas, &indexes),
            )?;
        }
        map.end()
    }
}

/// JS documentation of the native allocations table:
///
/// ```ignore
/// /**
///  * This variant is the original version of the table, before the memory address
///  * and threadId were added.
///  */
/// export type UnbalancedNativeAllocationsTable = {|
///   time: Milliseconds[],
///   // "weight" is used here rather than "bytes", so that this type will match the
///   // SamplesLikeTableShape.
///   weight: Bytes[],
///   weightType: 'bytes',
///   stack: Array<IndexIntoStackTable | null>,
///   length: number,
/// |};
///
/// /**
///  * The memory address and thread ID were added later.
///  */
/// export type BalancedNativeAllocationsTable = {|
///   ...UnbalancedNativeAllocationsTable,
///   memoryAddress: number[],
///   threadId: number[],
/// |};
/// ```
///
/// In this crate we always create a `BalancedNativeAllocationsTable`. We require
/// a memory address for each allocation / deallocation sample.
#[derive(Debug, Clone, Default)]
pub struct NativeAllocationsTable {
    /// The timstamps for each sample
    time: Vec<Timestamp>,
    /// The stack index for each sample
    stack: Vec<Option<usize>>,
    /// The size in bytes (positive for allocations, negative for deallocations) for each sample
    allocation_size: Vec<i64>,
    /// The memory address of the allocation for each sample
    allocation_address: Vec<u64>,
}

impl NativeAllocationsTable {
    /// Add a sample to the [`NativeAllocations`] table.
    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        self.time.push(timestamp);
        self.stack.push(stack_index);
        self.allocation_address.push(allocation_address);
        self.allocation_size.push(allocation_size);
    }
}

impl Serialize for NativeAllocationsTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.time.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("time", &self.time)?;
        map.serialize_entry("weight", &self.allocation_size)?;
        map.serialize_entry("weightType", &WeightType::Bytes)?;
        map.serialize_entry("stack", &self.stack)?;
        map.serialize_entry("memoryAddress", &self.allocation_address)?;

        // The threadId column is currently unused by the Firefox Profiler.
        // Fill the column with zeros because the type definitions require it to be a number.
        // A better alternative would be to use thread indexes or the threads' string TIDs.
        map.serialize_entry("threadId", &SerializableSingleValueColumn(0, len))?;

        map.serialize_entry("length", &len)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use assert_json_diff::assert_json_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn test_serialize_native_allocations() {
        // example of `nativeAllocations`:
        //
        // "nativeAllocations": {
        //     "time": [
        //         274364.1082197344,
        //         274364.17226073437,
        //         274364.2063027344,
        //         274364.2229277344,
        //         274364.44117773435,
        //         274366.4713027344,
        //         274366.48871973436,
        //         274366.6601777344,
        //         274366.6705107344
        //     ],
        //     "weight": [
        //         4096,
        //         -4096,
        //         4096,
        //         -4096,
        //         147456,
        //         4096,
        //         -4096,
        //         96,
        //         -96
        //     ],
        //     "weightType": "bytes",
        //     "stack": [
        //         71,
        //         88,
        //         119,
        //         138,
        //         null,
        //         171,
        //         190,
        //         210,
        //         214
        //     ],
        //     "memoryAddress": [
        //         4388749312,
        //         4388749312,
        //         4388749312,
        //         4388749312,
        //         4376330240,
        //         4388749312,
        //         4388749312,
        //         4377576256,
        //         4377576256
        //     ],
        //     "threadId": [
        //         0,
        //         0,
        //         0,
        //         0,
        //         0,
        //         0,
        //         0,
        //         0,
        //         0
        //     ],
        //     "length": 9
        // },

        let mut native_allocations_table = NativeAllocationsTable::default();
        native_allocations_table.add_sample(
            Timestamp::from_millis_since_reference(274_363.248_375),
            None,
            5969772544,
            147456,
        );

        assert_json_eq!(
            native_allocations_table,
            json!({
              "time": [
                274363.248375
              ],
              "weight": [
                147456
              ],
              "weightType": "bytes",
              "stack": [
                null
              ],
              "memoryAddress": [
                5969772544u64
              ],
              "threadId": [
                0
              ],
              "length": 1
            })
        );
    }
}
