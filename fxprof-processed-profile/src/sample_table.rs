use crate::cpu_delta::CpuDelta;
use crate::Timestamp;
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::fmt::{Display, Formatter};

// The Gecko Profiler records samples of what function was currently being executed, and the
// callstack that is associated with it. This is done at a fixed but configurable rate, e.g. every
// 1 millisecond. This table represents the minimal amount of information that is needed to
// represent that sampled function. Most of the entries are indices into other tables.
#[derive(Debug, Clone, Default)]
pub struct SampleTable {
    sample_type: WeightType,
    /// An optional weight array.
    ///
    /// If not present, then the weight is assumed to be 1. See the [WeightType] type for more
    /// information.
    sample_weights: Vec<i32>,
    sample_timestamps: Vec<Timestamp>,
    sample_stack_indexes: Vec<Option<usize>>,
    /// CPU usage value of the current thread. Its values are null only if the back-end fails to
    /// get the CPU usage from operating system.
    ///
    /// It's landed in Firefox 86, and it is optional because older profile versions may not have
    /// it or that feature could be disabled. No upgrader was written for this change because it's
    /// a completely new data source.
    sample_cpu_deltas: Vec<CpuDelta>,
}

/// Profile samples can come in a variety of forms and represent different information.
/// The Gecko Profiler by default uses sample counts, as it samples on a fixed interval.
/// These samples are all weighted equally by default, with a weight of one. However in
/// comparison profiles, some weights are negative, creating a "diff" profile.
///
/// In addition, tracing formats can fit into the sample-based format by reporting
/// the "self time" of the profile. Each of these "self time" samples would then
/// provide the weight, in duration. Currently, the tracing format assumes that
/// the timing comes in milliseconds (see 'tracing-ms') but if needed, microseconds
/// or nanoseconds support could be added.
///
/// e.g. The following tracing data could be represented as samples:
///
/// ```ignore
///     0 1 2 3 4 5 6 7 8 9 10
///     | | | | | | | | | | |
///     - - - - - - - - - - -
///     A A A A A A A A A A A
///         B B D D D D
///         C C E E E E
/// ```
/// This chart represents the self time.
/// ```ignore
///     0 1 2 3 4 5 6 7 8 9 10
///     | | | | | | | | | | |
///     A A C C E E E E A A A
/// ```
/// And finally this is what the samples table would look like.
/// ```ignore
///     SamplesTable = {
///       time:   [0,   2,   4, 8],
///       stack:  [A, ABC, ADE, A],
///       weight: [2,   2,   4, 3],
///     }
/// ```
///
/// JS type definition:
/// ```ignore
/// export type WeightType = 'samples' | 'tracing-ms' | 'bytes';
/// ```
///
/// Documentation and code from:
/// <https://github.com/firefox-devtools/profiler/blob/7bf02b3f747a33a8c166c533dc29304fde725517/src/types/profile.js#L127>
#[derive(Debug, Clone)]
pub enum WeightType {
    /// Each sample will have a weight of 1.
    Samples,
    /// Each sample will have a weight in terms of milliseconds.
    #[allow(dead_code)]
    TracingMs,
    /// Each sample will have a weight in terms of bytes allocated.
    Bytes,
}

impl Default for WeightType {
    fn default() -> Self {
        WeightType::Samples
    }
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

impl SampleTable {
    pub fn new() -> Self {
        Default::default()
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
        map.serialize_entry("stack", &self.sample_stack_indexes)?;
        map.serialize_entry("time", &self.sample_timestamps)?;
        map.serialize_entry("weight", &self.sample_weights)?;
        map.serialize_entry("weightType", &self.sample_type.to_string())?;
        map.serialize_entry("threadCPUDelta", &self.sample_cpu_deltas)?;
        map.end()
    }
}

/// js documentation on `NativeAllocations`:
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
///
/// /**
///  * Native allocations are recorded as a marker payload, but in profile processing they
///  * are moved to the Thread. This allows them to be part of the stack processing pipeline.
///  * Currently they include native allocations and deallocations. However, both
///  * of them are sampled independently, so they will be unbalanced if summed togther.
///  */
/// export type NativeAllocationsTable =
///   | UnbalancedNativeAllocationsTable
///   | BalancedNativeAllocationsTable;
/// ```
/// Example of `NativeAllocations` table:
#[derive(Debug, Clone)]
pub struct NativeAllocations {
    /// The timstamps for each sample
    time: Vec<Timestamp>,
    /// The weight for each sample
    weight: Vec<i32>,
    /// The type of sample
    weight_type: WeightType,
    /// The stack index for each sample
    stack: Vec<Option<usize>>,
    /// The memory address for each sample
    memory_address: Vec<Option<usize>>,
    /// The thread id for each sample
    thread_id: Vec<usize>,
}

impl Default for NativeAllocations {
    fn default() -> Self {
        Self {
            time: Vec::new(),
            weight: Vec::new(),
            weight_type: WeightType::Bytes,
            stack: Vec::new(),
            memory_address: Vec::new(),
            thread_id: Vec::new(),
        }
    }
}

impl NativeAllocations {
    /// Add a sample to the [`NativeAllocations`] table.
    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        stack_index: Option<usize>,
        memory_address: Option<usize>,
        thread_id: usize,
        weight: i32,
    ) {
        self.time.push(timestamp);
        self.stack.push(stack_index);
        self.memory_address.push(memory_address);
        self.thread_id.push(thread_id);
        self.weight.push(weight);
    }
}

impl Serialize for NativeAllocations {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.time.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("time", &self.time)?;
        map.serialize_entry("weight", &self.weight)?;
        map.serialize_entry("weightType", &self.weight_type.to_string())?;
        map.serialize_entry("stack", &self.stack)?;
        map.serialize_entry("memoryAddress", &self.memory_address)?;
        map.serialize_entry("threadId", &self.thread_id)?;
        map.serialize_entry("length", &len)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
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
        //         24940007,
        //         24940007,
        //         24940007,
        //         24940007,
        //         24965431,
        //         24940007,
        //         24940007,
        //         24939992,
        //         24939992
        //     ],
        //     "length": 9
        // },
        let native_allocations = r#"{
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
    5969772544
  ],
  "threadId": [
    24965427
  ],
  "length": 1
}"#;

        let mut native_allocations_table = NativeAllocations::default();
        native_allocations_table.add_sample(
            Timestamp::from_millis_since_reference(274_363.248_375),
            None,
            Some(5969772544),
            24965427,
            147456,
        );

        let serialized = serde_json::to_string_pretty(&native_allocations_table).unwrap();
        println!("{}", serialized);
        println!("{}", native_allocations);
        assert_eq!(serialized, native_allocations);
    }
}
