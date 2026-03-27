use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::global_lib_table::GlobalLibIndex;
use crate::resource_table::{ResourceIndex, ResourceTable};
use crate::source_table::SourceIndex;
use crate::string_table::StringHandle;

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    name_col: Vec<StringHandle>,
    source_col: Vec<Option<SourceIndex>>,
    start_line_col: Vec<Option<u32>>,
    start_column_col: Vec<Option<u32>>,
    resource_col: Vec<Option<ResourceIndex>>,
    flags_col: Vec<FrameFlags>,

    func_key_set: FastIndexSet<FuncKey>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncKey {
    pub name: StringHandle,
    pub source: Option<SourceIndex>,
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    pub lib: Option<GlobalLibIndex>,
    pub flags: FrameFlags,
}

impl FuncTable {
    pub fn index_for_func(
        &mut self,
        func_key: FuncKey,
        resource_table: &mut ResourceTable,
    ) -> FuncIndex {
        let (index, is_new) = self.func_key_set.insert_full(func_key);

        let func_index = FuncIndex(index.try_into().unwrap());
        if !is_new {
            return func_index;
        }

        let FuncKey {
            name,
            source,
            start_line,
            start_column,
            lib,
            flags,
        } = func_key;

        let resource = lib.map(|lib| resource_table.resource_for_lib(lib));

        self.name_col.push(name);
        self.source_col.push(source);
        self.start_line_col.push(start_line);
        self.start_column_col.push(start_column);
        self.resource_col.push(resource);
        self.flags_col.push(flags);

        func_index
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
        let len = self.name_col.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("name", &self.name_col)?;
        map.serialize_entry(
            "isJS",
            &SerializableFlagColumn(&self.flags_col, FrameFlags::IS_JS),
        )?;
        map.serialize_entry(
            "relevantForJS",
            &SerializableFlagColumn(&self.flags_col, FrameFlags::IS_RELEVANT_FOR_JS),
        )?;
        map.serialize_entry(
            "resource",
            &SerializableFuncTableResourceColumn(&self.resource_col),
        )?;
        map.serialize_entry("source", &self.source_col)?;
        map.serialize_entry("lineNumber", &self.start_line_col)?;
        map.serialize_entry("columnNumber", &self.start_column_col)?;
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
