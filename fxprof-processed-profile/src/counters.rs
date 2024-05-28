use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::{ProcessHandle, Timestamp};

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

impl<'a> Serialize for SerializableCounter<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("category", &self.counter.category)?;
        map.serialize_entry("name", &self.counter.name)?;
        map.serialize_entry("description", &self.counter.description)?;
        map.serialize_entry("mainThreadIndex", &self.main_thread_index)?;
        map.serialize_entry("pid", &self.counter.pid)?;
        map.serialize_entry("samples", &self.counter.samples)?;
        map.end()
    }
}

#[derive(Debug)]
struct CounterSamples {
    time: Vec<Timestamp>,
    number: Vec<u32>,
    count: Vec<f64>,
}

impl CounterSamples {
    pub fn new() -> Self {
        Self {
            time: Vec::new(),
            number: Vec::new(),
            count: Vec::new(),
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
    }
}

impl Serialize for CounterSamples {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.time.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("count", &self.count)?;
        map.serialize_entry("number", &self.number)?;
        map.serialize_entry("time", &self.time)?;
        map.end()
    }
}
