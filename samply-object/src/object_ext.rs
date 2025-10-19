use object::{Object, ObjectSection};
use uuid::Uuid;

use debugid::DebugId;

use samply_debugid::{CodeId, DebugIdExt, ElfBuildId};

pub trait ObjectExt<'data>: Object<'data> {
    /// Tries to obtain a CodeId for an object.
    ///
    /// This currently only handles mach-O and ELF.
    fn code_id(&self) -> Option<CodeId>;

    /// Tries to obtain a DebugId for an object. This uses the build ID, if available,
    /// and falls back to hashing the first page of the text section otherwise.
    /// Returns None on failure.
    fn debug_id(&self) -> Option<DebugId>;
}

// Blanket implementation
impl<'data, T: ?Sized> ObjectExt<'data> for T
where
    T: Object<'data>,
{
    fn code_id(&self) -> Option<CodeId> {
        // ELF
        if let Ok(Some(build_id)) = self.build_id() {
            return Some(CodeId::ElfBuildId(ElfBuildId::from_bytes(build_id)));
        }

        // mach-O
        if let Ok(Some(uuid)) = self.mach_uuid() {
            return Some(CodeId::MachoUuid(Uuid::from_bytes(uuid)));
        }

        None
    }

    fn debug_id(&self) -> Option<DebugId> {
        // Windows
        if let Ok(Some(pdb_info)) = self.pdb_info() {
            return Some(DebugId::from_guid_age(&pdb_info.guid(), pdb_info.age()).unwrap());
        }

        // ELF
        if let Ok(Some(build_id)) = self.build_id() {
            return Some(DebugId::from_identifier(build_id, self.is_little_endian()));
        }

        // mach-O
        if let Ok(Some(uuid)) = self.mach_uuid() {
            return Some(DebugId::from_uuid(Uuid::from_bytes(uuid)));
        }

        // We were not able to locate a build ID, so fall back to creating a synthetic
        // identifier from a hash of the first page of the ".text" (program code) section.
        if let Some(section) = self.section_by_name(".text") {
            let data_len = section.size().min(4096);
            if let Ok(Some(first_page_data)) = section.data_range(section.address(), data_len) {
                return Some(DebugId::from_text_first_page(
                    first_page_data,
                    self.is_little_endian(),
                ));
            }
        }

        None
    }
}
