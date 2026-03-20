use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::fast_hash_map::FastIndexSet;
use crate::string_table::StringHandle;

#[derive(Debug, Clone, Default)]
pub struct SourceTable {
    id_col: Vec<Option<StringHandle>>,
    file_path_col: Vec<StringHandle>,
    start_line_col: Vec<u32>,
    start_column_col: Vec<u32>,
    source_map_url_col: Vec<Option<StringHandle>>,

    source_key_set: FastIndexSet<SourceKey>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub id: Option<StringHandle>,
    pub file_path: StringHandle,
    pub start_line: u32,   // Use 1 if unsure
    pub start_column: u32, // Use 1 if unsure
    pub source_map_url: Option<StringHandle>,
}

impl SourceTable {
    pub fn index_for_source(&mut self, source_key: SourceKey) -> SourceIndex {
        let (index, is_new) = self.source_key_set.insert_full(source_key);

        let source_index = SourceIndex(index.try_into().unwrap());
        if !is_new {
            return source_index;
        }

        let SourceKey {
            id,
            file_path,
            start_line,
            start_column,
            source_map_url,
        } = source_key;

        self.id_col.push(id);
        self.file_path_col.push(file_path);
        self.start_line_col.push(start_line);
        self.start_column_col.push(start_column);
        self.source_map_url_col.push(source_map_url);

        source_index
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
        let len = self.id_col.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("uuid", &self.id_col)?;
        map.serialize_entry("filename", &self.file_path_col)?;
        // map.serialize_entry("startLine", &self.start_line_col)?;
        // map.serialize_entry("startColumn", &self.start_column_col)?;
        // map.serialize_entry("sourceMapURL", &self.source_map_url_col)?;
        map.end()
    }
}
