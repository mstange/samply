use std::hash::{BuildHasher, Hash, Hasher};

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::frame::FrameFlags;
use crate::resource_table::ResourceIndex;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::source_table::SourceIndex;
use crate::string_table::StringHandle;

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    set: ColumnarInterner<FuncCols>,
}

#[derive(Debug, Clone, Default)]
struct FuncCols {
    name: Vec<StringHandle>,
    source: Vec<Option<SourceIndex>>,
    start_line: Vec<Option<u32>>,
    start_column: Vec<Option<u32>>,
    resource: Vec<Option<ResourceIndex>>,
    flags: Vec<FrameFlags>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncKey {
    pub name: StringHandle,
    pub source: Option<SourceIndex>,
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    pub resource: Option<ResourceIndex>,
    pub flags: FrameFlags,
}

impl ColumnarStore for FuncCols {
    type Row = FuncKey;

    fn len(&self) -> usize {
        self.name.len()
    }

    fn hash_row<H: BuildHasher>(row: &FuncKey, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.name.hash(&mut h);
        row.source.hash(&mut h);
        row.start_line.hash(&mut h);
        row.start_column.hash(&mut h);
        row.resource.hash(&mut h);
        row.flags.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.name[i].hash(&mut h);
        self.source[i].hash(&mut h);
        self.start_line[i].hash(&mut h);
        self.start_column[i].hash(&mut h);
        self.resource[i].hash(&mut h);
        self.flags[i].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, i: usize, row: &FuncKey) -> bool {
        self.name[i] == row.name
            && self.source[i] == row.source
            && self.start_line[i] == row.start_line
            && self.start_column[i] == row.start_column
            && self.resource[i] == row.resource
            && self.flags[i] == row.flags
    }

    fn push(&mut self, row: FuncKey) {
        self.name.push(row.name);
        self.source.push(row.source);
        self.start_line.push(row.start_line);
        self.start_column.push(row.start_column);
        self.resource.push(row.resource);
        self.flags.push(row.flags);
    }
}

impl FuncTable {
    pub fn index_for_func(&mut self, func_key: FuncKey) -> FuncIndex {
        FuncIndex(self.set.insert(func_key))
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncIndex(u32);

impl Serialize for FuncIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl Serialize for FuncTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let cols = self.set.store();
        let len = self.set.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("name", &cols.name)?;
        map.serialize_entry(
            "isJS",
            &SerializableFlagColumn(&cols.flags, FrameFlags::IS_JS),
        )?;
        map.serialize_entry(
            "relevantForJS",
            &SerializableFlagColumn(&cols.flags, FrameFlags::IS_RELEVANT_FOR_JS),
        )?;
        map.serialize_entry(
            "resource",
            &SerializableFuncTableResourceColumn(&cols.resource),
        )?;
        map.serialize_entry("source", &cols.source)?;
        map.serialize_entry("lineNumber", &cols.start_line)?;
        map.serialize_entry("columnNumber", &cols.start_column)?;
        map.serialize_entry(
            "originalLocation",
            &SerializableSingleValueColumn(Option::<u32>::None, len),
        )?;
        map.end()
    }
}

struct SerializableFuncTableResourceColumn<'a>(&'a [Option<ResourceIndex>]);

impl Serialize for SerializableFuncTableResourceColumn<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for resource in self.0 {
            match resource {
                Some(resource) => seq.serialize_element(&resource)?,
                None => seq.serialize_element(&-1)?,
            }
        }
        seq.end()
    }
}

pub struct SerializableFlagColumn<'a>(&'a [FrameFlags], FrameFlags);

impl Serialize for SerializableFlagColumn<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for item_flags in self.0 {
            seq.serialize_element(&item_flags.contains(self.1))?;
        }
        seq.end()
    }
}
