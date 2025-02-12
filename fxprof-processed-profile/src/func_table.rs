use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::frame::FrameFlags;
use crate::resource_table::ResourceIndex;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::thread_string_table::ThreadInternalStringIndex;

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    names: Vec<ThreadInternalStringIndex>,
    files: Vec<Option<ThreadInternalStringIndex>>,
    resources: Vec<Option<ResourceIndex>>,
    flags: Vec<FrameFlags>,
    func_key_to_func_index: FastHashMap<FuncKey, FuncIndex>,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct FuncKey {
    name: ThreadInternalStringIndex,
    file: Option<ThreadInternalStringIndex>,
    resource: Option<ResourceIndex>,
    flags: FrameFlags,
}

impl FuncTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_func(
        &mut self,
        name: ThreadInternalStringIndex,
        file: Option<ThreadInternalStringIndex>,
        resource: Option<ResourceIndex>,
        flags: FrameFlags,
    ) -> FuncIndex {
        let key = FuncKey {
            name,
            file,
            resource,
            flags,
        };
        if let Some(index) = self.func_key_to_func_index.get(&key) {
            return *index;
        }

        let func_index = FuncIndex(u32::try_from(self.names.len()).unwrap());
        self.names.push(name);
        self.files.push(file);
        self.resources.push(resource);
        self.flags.push(flags);
        self.func_key_to_func_index.insert(key, func_index);
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
