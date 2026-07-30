use std::hash::{BuildHasher, Hash, Hasher};
use std::io::Write;

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use crate::frame::FrameFlags;
use crate::resource_table::ResourceIndex;
use crate::source_table::SourceIndex;
use crate::string_table::StringHandle;
use crate::writer::{SplitOutObjectBody, Writer};

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncIndex(pub(crate) i32);

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
        FuncIndex(self.set.insert(func_key) as i32)
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        let cols = self.set.store();
        let len = self.set.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("name")?;
            w.array(|w| {
                for n in &cols.name {
                    n.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("isJS")?;
            w.array(|w| {
                for flags in &cols.flags {
                    w.bool_value(flags.contains(FrameFlags::IS_JS))?;
                }
                Ok(())
            })?;
            w.name("relevantForJS")?;
            w.array(|w| {
                for flags in &cols.flags {
                    w.bool_value(flags.contains(FrameFlags::IS_RELEVANT_FOR_JS))?;
                }
                Ok(())
            })?;
            w.name("resource")?;
            w.array(|w| {
                for r in &cols.resource {
                    match r {
                        Some(r) => r.write_json(w)?,
                        None => w.number_value(-1)?,
                    }
                }
                Ok(())
            })?;
            w.name("source")?;
            w.array(|w| {
                for s in &cols.source {
                    match s {
                        Some(s) => s.write_json(w)?,
                        None => w.null_value()?,
                    }
                }
                Ok(())
            })?;
            w.name("lineNumber")?;
            w.optional_number_array(&cols.start_line)?;
            w.name("columnNumber")?;
            w.optional_number_array(&cols.start_column)?;
            w.name("originalLocation")?;
            w.null_array(len)
        })
    }
}

impl<'p> SplitOutObjectBody<'p> for &'p FuncTable {
    fn write_body<W: Write>(self, w: &mut Writer<'_, 'p, W>) -> std::io::Result<()> {
        self.write_json(w)
    }
}
