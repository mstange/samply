use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::frame::FrameFlags;
use crate::resource_table::ResourceIndex;
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::thread_string_table::ThreadInternalStringIndex;

#[derive(Debug, Clone, Default)]
pub struct FuncTable {
    names: Vec<ThreadInternalStringIndex>,
    resources: Vec<Option<ResourceIndex>>,
    flags: Vec<FrameFlags>,
    func_name_and_resource_and_flags_to_func_index:
        FastHashMap<(ThreadInternalStringIndex, Option<ResourceIndex>, FrameFlags), usize>,
    contains_js_function: bool,
}

impl FuncTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_func(
        &mut self,
        name: ThreadInternalStringIndex,
        resource: Option<ResourceIndex>,
        flags: FrameFlags,
    ) -> FuncIndex {
        let func_index = *self
            .func_name_and_resource_and_flags_to_func_index
            .entry((name, resource, flags))
            .or_insert_with(|| {
                let func_index = self.names.len();
                self.names.push(name);
                self.resources.push(resource);
                self.flags.push(flags);
                func_index
            });
        if flags.intersects(FrameFlags::IS_JS | FrameFlags::IS_RELEVANT_FOR_JS) {
            self.contains_js_function = true;
        }
        FuncIndex(func_index as u32)
    }

    pub fn contains_js_function(&self) -> bool {
        self.contains_js_function
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
        map.serialize_entry("fileName", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("lineNumber", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry("columnNumber", &SerializableSingleValueColumn((), len))?;
        map.end()
    }
}

struct SerializableFuncTableResourceColumn<'a>(&'a [Option<ResourceIndex>]);

impl<'a> Serialize for SerializableFuncTableResourceColumn<'a> {
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

impl<'a> Serialize for SerializableFlagColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for item_flags in self.0 {
            seq.serialize_element(&item_flags.contains(self.1))?;
        }
        seq.end()
    }
}
