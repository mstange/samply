use serde::ser::{Serialize, SerializeSeq, Serializer};

use crate::Timestamp;

pub struct SerializableSingleValueColumn<T: Serialize>(pub T, pub usize);

impl<T: Serialize> Serialize for SerializableSingleValueColumn<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.1))?;
        for _ in 0..self.1 {
            seq.serialize_element(&self.0)?;
        }
        seq.end()
    }
}

pub struct SerializableOptionalTimestampColumn<'a>(pub &'a [Option<Timestamp>]);

impl<'a> Serialize for SerializableOptionalTimestampColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for timestamp in self.0 {
            match timestamp {
                Some(timestamp) => seq.serialize_element(&timestamp)?,
                None => seq.serialize_element(&0.0)?,
            }
        }
        seq.end()
    }
}
