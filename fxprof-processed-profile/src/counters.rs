use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::serialization_helpers::SliceWithPermutation;
use crate::timestamp::{
    SerializableTimestampSliceAsDeltas, SerializableTimestampSliceAsDeltasWithPermutation,
};
use crate::{GraphColor, ProcessHandle, Timestamp};

/// A counter. Can be created with [`Profile::add_counter`](crate::Profile::add_counter).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CounterHandle(pub(crate) usize);

#[derive(Debug)]
pub struct Counter {
    name: String,
    category: String,
    description: String,
    process: ProcessHandle,
    pid: String,
    samples: CounterSamples,
    color: Option<GraphColor>,
}

impl Counter {
    pub fn new(
        name: &str,
        category: &str,
        description: &str,
        process: ProcessHandle,
        pid: &str,
    ) -> Self {
        Counter {
            name: name.to_owned(),
            category: category.to_owned(),
            description: description.to_owned(),
            process,
            pid: pid.to_owned(),
            samples: CounterSamples::new(),
            color: None,
        }
    }

    pub fn process(&self) -> ProcessHandle {
        self.process
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        value_delta: f64,
        number_of_operations_delta: u32,
    ) {
        self.samples
            .add_sample(timestamp, value_delta, number_of_operations_delta)
    }

    pub fn set_color(&mut self, color: GraphColor) {
        self.color = Some(color);
    }

    pub fn as_serializable(&self, main_thread_index: usize) -> impl Serialize + '_ {
        SerializableCounter {
            counter: self,
            main_thread_index,
        }
    }
}

struct SerializableCounter<'a> {
    counter: &'a Counter,
    /// The index of the main thread for the counter's process in the profile threads list.
    main_thread_index: usize,
}

impl Serialize for SerializableCounter<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("category", &self.counter.category)?;
        map.serialize_entry("name", &self.counter.name)?;
        map.serialize_entry("description", &self.counter.description)?;
        map.serialize_entry("mainThreadIndex", &self.main_thread_index)?;
        map.serialize_entry("pid", &self.counter.pid)?;
        map.serialize_entry("samples", &self.counter.samples)?;
        if let Some(color) = self.counter.color {
            map.serialize_entry("color", &color)?;
        }
        map.end()
    }
}

#[derive(Debug)]
struct CounterSamples {
    time: Vec<Timestamp>,
    number: Vec<u32>,
    count: Vec<f64>,

    is_sorted_by_time: bool,
    last_sample_timestamp: Timestamp,
}

impl CounterSamples {
    pub fn new() -> Self {
        Self {
            time: Vec::new(),
            number: Vec::new(),
            count: Vec::new(),

            is_sorted_by_time: true,
            last_sample_timestamp: Timestamp::from_nanos_since_reference(0),
        }
    }

    pub fn add_sample(
        &mut self,
        timestamp: Timestamp,
        value_delta: f64,
        number_of_operations_delta: u32,
    ) {
        self.time.push(timestamp);
        self.count.push(value_delta);
        self.number.push(number_of_operations_delta);

        if timestamp < self.last_sample_timestamp {
            self.is_sorted_by_time = false;
        }
        self.last_sample_timestamp = timestamp;
    }
}

impl Serialize for CounterSamples {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.time.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;

        if self.is_sorted_by_time {
            map.serialize_entry("count", &self.count)?;
            map.serialize_entry("number", &self.number)?;
            map.serialize_entry(
                "timeDeltas",
                &SerializableTimestampSliceAsDeltas(&self.time),
            )?;
        } else {
            let mut indexes: Vec<usize> = (0..self.time.len()).collect();
            indexes.sort_unstable_by_key(|index| self.time[*index]);
            map.serialize_entry("count", &SliceWithPermutation(&self.count, &indexes))?;
            map.serialize_entry("number", &SliceWithPermutation(&self.number, &indexes))?;
            map.serialize_entry(
                "timeDeltas",
                &SerializableTimestampSliceAsDeltasWithPermutation(&self.time, &indexes),
            )?;
        }

        map.end()
    }
}
