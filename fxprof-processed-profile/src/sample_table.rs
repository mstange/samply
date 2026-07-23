use std::fmt::{Display, Formatter};
use std::io::Write;

use crate::cpu_delta::CpuDelta;
use crate::timestamp::{
    write_timestamps_as_deltas, write_timestamps_as_deltas_with_permutation, Timestamp,
};
use crate::writer::Writer;
use crate::StackHandle;

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
    sample_stack_indexes: Vec<Option<StackHandle>>,
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

impl WeightType {
    pub(crate) fn as_json_str(&self) -> &'static str {
        match self {
            WeightType::Samples => "samples",
            WeightType::TracingMs => "tracing-ms",
            WeightType::Bytes => "bytes",
        }
    }
}

impl Display for WeightType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_json_str())
    }
}

fn write_stack_column<W: Write>(
    w: &mut Writer<W>,
    stacks: &[Option<StackHandle>],
) -> std::io::Result<()> {
    w.array(|w| {
        for s in stacks {
            match s {
                Some(s) => w.number_value(s.0)?,
                None => w.null_value()?,
            }
        }
        Ok(())
    })
}

fn write_stack_column_permuted<W: Write>(
    w: &mut Writer<W>,
    stacks: &[Option<StackHandle>],
    indexes: &[usize],
) -> std::io::Result<()> {
    w.array(|w| {
        for &i in indexes {
            match stacks[i] {
                Some(s) => w.number_value(s.0)?,
                None => w.null_value()?,
            }
        }
        Ok(())
    })
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
        stack_index: Option<StackHandle>,
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

    pub fn with_remapped_stacks(mut self, old_stack_to_new_stack: &[Option<StackHandle>]) -> Self {
        self.sample_stack_indexes = self
            .sample_stack_indexes
            .into_iter()
            .map(|stack| match stack {
                Some(s) => old_stack_to_new_stack[s.0 as usize],
                None => None,
            })
            .collect();
        self
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let len = self.sample_timestamps.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("weightType")?;
            w.string_value(self.sample_weight_type.as_json_str())?;

            if self.is_sorted_by_time {
                w.name("stack")?;
                write_stack_column(w, &self.sample_stack_indexes)?;
                w.name("timeDeltas")?;
                write_timestamps_as_deltas(w, &self.sample_timestamps)?;
                w.name("weight")?;
                w.number_array(&self.sample_weights)?;
                w.name("threadCPUDelta")?;
                w.array(|w| {
                    for cd in &self.sample_cpu_deltas {
                        cd.write_json(w)?;
                    }
                    Ok(())
                })?;
            } else {
                let mut indexes: Vec<usize> = (0..self.sample_timestamps.len()).collect();
                indexes.sort_unstable_by_key(|index| self.sample_timestamps[*index]);
                w.name("stack")?;
                write_stack_column_permuted(w, &self.sample_stack_indexes, &indexes)?;
                w.name("timeDeltas")?;
                write_timestamps_as_deltas_with_permutation(w, &self.sample_timestamps, &indexes)?;
                w.name("weight")?;
                w.array(|w| {
                    for &i in &indexes {
                        w.number_value(self.sample_weights[i])?;
                    }
                    Ok(())
                })?;
                w.name("threadCPUDelta")?;
                w.array(|w| {
                    for &i in &indexes {
                        self.sample_cpu_deltas[i].write_json(w)?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
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
    stack: Vec<Option<StackHandle>>,
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
        stack_index: Option<StackHandle>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        self.time.push(timestamp);
        self.stack.push(stack_index);
        self.allocation_address.push(allocation_address);
        self.allocation_size.push(allocation_size);
    }

    pub fn with_remapped_stacks(mut self, old_stack_to_new_stack: &[Option<StackHandle>]) -> Self {
        self.stack = self
            .stack
            .into_iter()
            .map(|stack| match stack {
                Some(s) => old_stack_to_new_stack[s.0 as usize],
                None => None,
            })
            .collect();
        self
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let len = self.time.len();
        w.object(|w| {
            w.name("time")?;
            w.array(|w| {
                for t in &self.time {
                    t.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("weight")?;
            w.number_array(&self.allocation_size)?;
            w.name("weightType")?;
            w.string_value(WeightType::Bytes.as_json_str())?;
            w.name("stack")?;
            write_stack_column(w, &self.stack)?;
            w.name("memoryAddress")?;
            w.number_array(&self.allocation_address)?;
            // The threadId column is currently unused by the Firefox Profiler.
            // Fill the column with zeros because the type definitions require it to be a number.
            // A better alternative would be to use thread indexes or the threads' string TIDs.
            w.name("threadId")?;
            w.array(|w| {
                for _ in 0..len {
                    w.number_value(0u32)?;
                }
                Ok(())
            })?;
            w.name("length")?;
            w.number_value(len)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use struson::writer::{JsonStreamWriter, JsonWriter};

    fn to_json(table: &NativeAllocationsTable) -> serde_json::Value {
        let mut buf = Vec::new();
        let mut json = JsonStreamWriter::new(&mut buf);
        let mut ctx = Writer { json: &mut json };
        table.write_json(&mut ctx).unwrap();
        json.finish_document().unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    #[test]
    fn test_serialize_native_allocations() {
        // example of `nativeAllocations`:
        //
        // "nativeAllocations": {
        //     "time": [ ... ],
        //     "weight": [ ... ],
        //     "weightType": "bytes",
        //     "stack": [ ... ],
        //     "memoryAddress": [ ... ],
        //     "threadId": [ ... ],
        //     "length": ...
        // }
        let mut native_allocations_table = NativeAllocationsTable::default();
        native_allocations_table.add_sample(
            Timestamp::from_millis_since_reference(274_363.248_375),
            None,
            5969772544,
            147456,
        );

        insta::assert_json_snapshot!(to_json(&native_allocations_table), @r#"
        {
          "length": 1,
          "memoryAddress": [
            5969772544
          ],
          "stack": [
            null
          ],
          "threadId": [
            0
          ],
          "time": [
            274363.248375
          ],
          "weight": [
            147456
          ],
          "weightType": "bytes"
        }
        "#);
    }
}
