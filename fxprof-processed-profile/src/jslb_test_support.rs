//! Helpers for tests which inspect JSLB output.
//!
//! These resolve slab references as they walk the document, so that tests can
//! say what they actually care about - "this column is an Int32Array slab
//! containing these values" - without also pinning down which objects we
//! choose to split out into their own JSON slab.

use json_slabs::{ParsedFile, SlabPlaceholder, SLAB_REF_KEY};
use serde_json::Value;

/// Look up an object by its path from the root of the JSLB document, e.g.
/// `["shared", "funcTable"]`. Any object along the way may be a reference to
/// a separate JSON slab; if it is, we follow the reference.
pub fn object_at_path(file: &ParsedFile, path: &[&str]) -> Value {
    let mut current: Value = serde_json::from_slice(file.root_json_bytes()).unwrap();
    for key in path {
        let mut next = current
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} at this point in the document"))
            .clone();
        if let Some(p) = slab_ref(&next) {
            next = serde_json::from_slice(file.read_subjson_bytes(p).unwrap()).unwrap();
        }
        current = next;
    }
    current
}

/// The slab which holds `object[column]`, for a column which is stored as a
/// typed array.
pub fn column_slab(object: &Value, column: &str) -> SlabPlaceholder {
    slab_ref(&object[column])
        .unwrap_or_else(|| panic!("column {column:?} should be a slab reference"))
}

fn slab_ref(value: &Value) -> Option<SlabPlaceholder> {
    let index = value.get(SLAB_REF_KEY)?.as_u64()?;
    Some(SlabPlaceholder(index as usize))
}
