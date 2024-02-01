use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::cpu_delta::CpuDelta;
use crate::Timestamp;

/// How to weight an individual sample.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub enum WeightType {
    /// Weight by sample count.
    Samples,
    /// Time in milliseconds.
    TracingMs,
    /// Size in bytes.
    Bytes,
}

impl Default for WeightType {
    fn default() -> Self {
        Self::Samples
    }
}

impl Serialize for WeightType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            WeightType::Samples => "samples".serialize(serializer),
            WeightType::TracingMs => "tracing-ms".serialize(serializer),
            WeightType::Bytes => "bytes".serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SampleTable {
    sample_weights: Vec<i32>,
    sample_weight_type: WeightType,
    sample_timestamps: Vec<Timestamp>,
    sample_stack_indexes: Vec<Option<usize>>,
    sample_cpu_deltas: Vec<CpuDelta>,
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
        map.serialize_entry("stack", &self.sample_stack_indexes)?;
        map.serialize_entry("time", &self.sample_timestamps)?;
        map.serialize_entry("weight", &self.sample_weights)?;
        map.serialize_entry("weightType", &self.sample_weight_type)?;
        map.serialize_entry("threadCPUDelta", &self.sample_cpu_deltas)?;
        map.end()
    }
}
