use std::sync::Arc;

use debugid::{CodeId, DebugId};
use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::library_info::SymbolTable;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Lib {
    pub name: String,
    pub debug_name: String,
    pub path: String,
    pub debug_path: String,
    pub arch: Option<String>,
    pub debug_id: DebugId,
    pub code_id: Option<CodeId>,
    pub symbol_table: Option<Arc<SymbolTable>>,
}

impl Serialize for Lib {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let breakpad_id = self.debug_id.breakpad().to_string();
        let code_id = self.code_id.as_ref().map(|cid| cid.to_string());
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("path", &self.path)?;
        map.serialize_entry("debugName", &self.debug_name)?;
        map.serialize_entry("debugPath", &self.debug_path)?;
        map.serialize_entry("breakpadId", &breakpad_id)?;
        map.serialize_entry("codeId", &code_id)?;
        map.serialize_entry("arch", &self.arch)?;
        map.end()
    }
}
