use std::hash::{BuildHasher, Hash, Hasher};

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::string_table::StringHandle;

#[derive(Debug, Clone, Default)]
pub struct SourceTable {
    set: ColumnarInterner<SourceCols>,
}

#[derive(Debug, Clone, Default)]
struct SourceCols {
    id: Vec<Option<StringHandle>>,
    file_path: Vec<StringHandle>,
    start_line: Vec<u32>,
    start_column: Vec<u32>,
    source_map_url: Vec<Option<StringHandle>>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub id: Option<StringHandle>,
    pub file_path: StringHandle,
    pub start_line: u32,   // Use 1 if unsure
    pub start_column: u32, // Use 1 if unsure
    pub source_map_url: Option<StringHandle>,
}

impl ColumnarStore for SourceCols {
    type Row = SourceKey;

    fn len(&self) -> usize {
        self.file_path.len()
    }

    fn hash_row<H: BuildHasher>(row: &SourceKey, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.id.hash(&mut h);
        row.file_path.hash(&mut h);
        row.start_line.hash(&mut h);
        row.start_column.hash(&mut h);
        row.source_map_url.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.id[i].hash(&mut h);
        self.file_path[i].hash(&mut h);
        self.start_line[i].hash(&mut h);
        self.start_column[i].hash(&mut h);
        self.source_map_url[i].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, i: usize, row: &SourceKey) -> bool {
        self.id[i] == row.id
            && self.file_path[i] == row.file_path
            && self.start_line[i] == row.start_line
            && self.start_column[i] == row.start_column
            && self.source_map_url[i] == row.source_map_url
    }

    fn push(&mut self, row: SourceKey) {
        self.id.push(row.id);
        self.file_path.push(row.file_path);
        self.start_line.push(row.start_line);
        self.start_column.push(row.start_column);
        self.source_map_url.push(row.source_map_url);
    }
}

impl SourceTable {
    pub fn index_for_source(&mut self, source_key: SourceKey) -> SourceIndex {
        SourceIndex(self.set.insert(source_key))
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SourceIndex(u32);

impl Serialize for SourceIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl Serialize for SourceTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let cols = self.set.store();
        let len = self.set.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("id", &cols.id)?;
        map.serialize_entry("filename", &cols.file_path)?;
        map.serialize_entry("startLine", &cols.start_line)?;
        map.serialize_entry("startColumn", &cols.start_column)?;
        map.serialize_entry("sourceMapURL", &cols.source_map_url)?;
        map.serialize_entry(
            "content",
            &SerializableSingleValueColumn(Option::<&str>::None, len),
        )?;
        map.end()
    }
}
