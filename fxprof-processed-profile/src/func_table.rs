use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::fast_hash_map::FastIndexSet;
use crate::frame::FrameFlags;
use crate::global_lib_table::GlobalLibIndex;
use crate::resource_table::{ResourceIndex, ResourceTable};
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::string_table::StringHandle;

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    names: Vec<StringHandle>,
    files: Vec<Option<StringHandle>>,
    resources: Vec<Option<ResourceIndex>>,
    flags: Vec<FrameFlags>,

    func_key_set: FastIndexSet<FuncKey>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FuncKey {
    pub name: StringHandle,
    pub file_path: Option<StringHandle>,
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
            file_path,
            lib,
            flags,
        } = func_key;

        let resource = lib.map(|lib| resource_table.resource_for_lib(lib));

        self.names.push(name);
        self.files.push(file_path);
        self.resources.push(resource);
        self.flags.push(flags);

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
        let len = self.names.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("name", &self.names)?;
        map.serialize_entry(
            "isJS",
            &SerializableFlagColumn(&self.flags, FrameFlags::IS_JS),
        )?;
        map.serialize_entry(
            "relevantForJS",
            &SerializableFlagColumn(&self.flags, FrameFlags::IS_RELEVANT_FOR_JS),
        )?;
        map.serialize_entry(
            "resource",
            &SerializableFuncTableResourceColumn(&self.resources),
        )?;
        map.serialize_entry("fileName", &self.files)?;
        map.serialize_entry("lineNumber", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("columnNumber", &SerializableSingleValueColumn((), len))?;
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
