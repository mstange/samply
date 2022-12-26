use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::fast_hash_map::FastHashMap;
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable};
use crate::serialization_helpers::SerializableSingleValueColumn;
use crate::thread_string_table::ThreadInternalStringIndex;
use crate::thread_string_table::ThreadStringTable;

#[derive(Debug, Clone, Default)]
pub struct ResourceTable {
    resource_libs: Vec<GlobalLibIndex>,
    resource_names: Vec<ThreadInternalStringIndex>,
    lib_to_resource: FastHashMap<GlobalLibIndex, ResourceIndex>,
}

impl ResourceTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn resource_for_lib(
        &mut self,
        lib_index: GlobalLibIndex,
        global_libs: &GlobalLibTable,
        thread_string_table: &mut ThreadStringTable,
    ) -> ResourceIndex {
        let resource_libs = &mut self.resource_libs;
        let resource_names = &mut self.resource_names;
        *self.lib_to_resource.entry(lib_index).or_insert_with(|| {
            let resource = ResourceIndex(resource_libs.len() as u32);
            let lib_name = &global_libs.get_lib(lib_index).unwrap().name;
            resource_libs.push(lib_index);
            resource_names.push(thread_string_table.index_for_string(lib_name));
            resource
        })
    }
}

impl Serialize for ResourceTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const RESOURCE_TYPE_LIB: u32 = 1;
        let len = self.resource_libs.len();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("lib", &self.resource_libs)?;
        map.serialize_entry("name", &self.resource_names)?;
        map.serialize_entry("host", &SerializableSingleValueColumn((), len))?;
        map.serialize_entry(
            "type",
            &SerializableSingleValueColumn(RESOURCE_TYPE_LIB, len),
        )?;
        map.end()
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ResourceIndex(u32);

impl Serialize for ResourceIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}
