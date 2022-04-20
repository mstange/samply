use debugid::DebugId;
use object::{Object, ObjectSection, SectionKind};
use uuid::Uuid;

pub trait DebugIdElfExt {
    /// Creates a DebugId from some identifier. The identifier could be
    /// an ELF build ID, or a hash derived from the text section.
    /// The `little_endian` argument specifies whether the object file
    /// is targeting a little endian architecture.
    fn from_identifier(identifier: &[u8], little_endian: bool) -> Self;

    /// Creates a DebugId from a hash of the first 4096 bytes of the .text section.
    /// The `little_endian` argument specifies whether the object file
    /// is targeting a little endian architecture.
    fn from_text_first_page(text_first_page: &[u8], little_endian: bool) -> Self;
}

impl DebugIdElfExt for DebugId {
    fn from_identifier(identifier: &[u8], little_endian: bool) -> Self {
        // Make sure that we have exactly 16 bytes available, either truncate or fill
        // the remainder with zeros.
        // ELF build IDs are usually 20 bytes, so if the identifier is an ELF build ID
        // then we're performing a lossy truncation.
        let mut d = [0u8; 16];
        let shared_len = identifier.len().min(d.len());
        d[0..shared_len].copy_from_slice(&identifier[0..shared_len]);

        // Pretend that the build ID was stored as a UUID with u32 u16 u16 fields inside
        // the file. Parse those fields in the endianness of the file. Then use
        // Uuid::from_fields to serialize them as big endian.
        // For ELF build IDs this is a bit silly, because ELF build IDs aren't actually
        // field-based UUIDs, but this is what the tools in the breakpad and
        // sentry/symbolic universe do, so we do the same for compatibility with those
        // tools.
        let (d1, d2, d3) = if little_endian {
            (
                u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
                u16::from_le_bytes([d[4], d[5]]),
                u16::from_le_bytes([d[6], d[7]]),
            )
        } else {
            (
                u32::from_be_bytes([d[0], d[1], d[2], d[3]]),
                u16::from_be_bytes([d[4], d[5]]),
                u16::from_be_bytes([d[6], d[7]]),
            )
        };
        let uuid = Uuid::from_fields(d1, d2, d3, &d[8..16])
            .expect("This always succeeds because we're passing exactly 8 bytes for d4");
        DebugId::from_uuid(uuid)
    }

    // This algorithm XORs 16-byte chunks directly into a 16-byte buffer.
    fn from_text_first_page(text_first_page: &[u8], little_endian: bool) -> Self {
        const UUID_SIZE: usize = 16;
        const PAGE_SIZE: usize = 4096;
        let mut hash = [0; UUID_SIZE];
        for (i, byte) in text_first_page.iter().cloned().take(PAGE_SIZE).enumerate() {
            hash[i % UUID_SIZE] ^= byte;
        }
        DebugId::from_identifier(&hash, little_endian)
    }
}

/// Tries to obtain a DebugId for an object. This uses the build ID, if available,
/// and falls back to hashing the first page of the text section otherwise.
/// Returns None on failure.
pub fn debug_id_for_object<'data: 'file, 'file>(
    obj: &'file impl Object<'data, 'file>,
) -> Option<DebugId> {
    // Windows
    if let Ok(Some(pdb_info)) = obj.pdb_info() {
        return Some(DebugId::from_guid_age(&pdb_info.guid(), pdb_info.age()).unwrap());
    }

    // ELF
    if let Ok(Some(build_id)) = obj.build_id() {
        return Some(DebugId::from_identifier(build_id, obj.is_little_endian()));
    }

    // mach-O
    if let Ok(Some(uuid)) = obj.mach_uuid() {
        return Some(DebugId::from_uuid(Uuid::from_bytes(uuid)));
    }

    // We were not able to locate a build ID, so fall back to creating a synthetic
    // identifier from a hash of the first page of the "text" (program code) section.
    if let Some(section) = obj
        .sections()
        .find(|section| section.kind() == SectionKind::Text)
    {
        let data_len = section.size().min(4096);
        if let Ok(Some(first_page_data)) = section.data_range(section.address(), data_len) {
            return Some(DebugId::from_text_first_page(
                first_page_data,
                obj.is_little_endian(),
            ));
        }
    }

    None
}
