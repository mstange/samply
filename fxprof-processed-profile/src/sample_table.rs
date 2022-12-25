use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::cpu_delta::CpuDelta;
use crate::Timestamp;

#[derive(Debug, Clone, Default)]
pub struct SampleTable {
    sample_weights: Vec<i32>,
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
        map.serialize_entry("weightType", &"samples")?;
        map.serialize_entry("threadCPUDelta", &self.sample_cpu_deltas)?;
        map.end()
    }
}
