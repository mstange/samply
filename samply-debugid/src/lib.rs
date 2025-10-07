mod codeid;
mod debugid;

pub use codeid::{code_id_for_object, CodeId, ElfBuildId, PeCodeId};
pub use debugid::{debug_id_for_object, DebugIdExt};
