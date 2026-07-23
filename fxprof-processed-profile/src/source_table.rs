use std::hash::{BuildHasher, Hash, Hasher};
use std::io::Write;

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::string_table::StringHandle;
use crate::writer::Writer;

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

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let cols = self.set.store();
        let len = self.set.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("id")?;
            w.array(|w| {
                for id in &cols.id {
                    StringHandle::write_optional(*id, w)?;
                }
                Ok(())
            })?;
            w.name("filename")?;
            w.array(|w| {
                for fp in &cols.file_path {
                    fp.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("startLine")?;
            w.number_array(&cols.start_line)?;
            w.name("startColumn")?;
            w.number_array(&cols.start_column)?;
            w.name("sourceMapURL")?;
            w.array(|w| {
                for url in &cols.source_map_url {
                    StringHandle::write_optional(*url, w)?;
                }
                Ok(())
            })?;
            w.name("content")?;
            w.null_array(len)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SourceIndex(u32);

impl SourceIndex {
    pub(crate) fn write_json<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.number_value(self.0)
    }
}
