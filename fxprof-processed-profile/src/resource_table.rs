use std::io::Write;

use crate::fast_hash_map::FastHashMap;
use crate::global_lib_table::GlobalLibIndex;
use crate::string_table::StringHandle;
use crate::writer::Writer;

#[derive(Debug, Clone, Default)]
pub struct ResourceTable {
    resource_libs: Vec<GlobalLibIndex>,
    resource_names: Vec<StringHandle>,
    lib_to_resource: FastHashMap<GlobalLibIndex, ResourceIndex>,
}

impl ResourceTable {
    pub fn resource_for_lib(&mut self, lib_index: GlobalLibIndex) -> ResourceIndex {
        let resource_libs = &mut self.resource_libs;
        let resource_names = &mut self.resource_names;
        *self.lib_to_resource.entry(lib_index).or_insert_with(|| {
            let resource = ResourceIndex(resource_libs.len() as u32);
            resource_libs.push(lib_index);
            resource_names.push(lib_index.name_string_index());
            resource
        })
    }

    pub(crate) fn write_json<W: Write>(&self, w: &mut Writer<W>) -> std::io::Result<()> {
        const RESOURCE_TYPE_LIB: u32 = 1;
        let len = self.resource_libs.len();
        w.object(|w| {
            w.name("length")?;
            w.number_value(len)?;
            w.name("lib")?;
            w.array(|w| {
                for lib in &self.resource_libs {
                    lib.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("name")?;
            w.array(|w| {
                for name in &self.resource_names {
                    name.write_json(w)?;
                }
                Ok(())
            })?;
            w.name("host")?;
            w.null_array(len)?;
            w.name("type")?;
            w.array(|w| {
                for _ in 0..len {
                    w.number_value(RESOURCE_TYPE_LIB)?;
                }
                Ok(())
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ResourceIndex(u32);

impl ResourceIndex {
    pub(crate) fn write_json<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.number_value(self.0)
    }
}
