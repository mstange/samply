use debugid::DebugId;
use object::{FileFlags, Object, ObjectSection};
use uuid::Uuid;

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

    /// The "relative address base" is the base address which [`LookupAddress::Relative`]
    /// addresses are relative to. You start with an SVMA (a stated virtual memory address),
    /// you subtract the relative address base, and out comes a relative address.
    ///
    /// This function computes that base address. It is defined as follows:
    ///
    ///  - For Windows binaries, the base address is the "image base address".
    ///  - For mach-O binaries, the base address is the vmaddr of the __TEXT segment.
    ///  - For ELF binaries, the base address is the vmaddr of the *first* segment,
    ///    i.e. the vmaddr of the first "LOAD" ELF command.
    ///
    /// In many cases, this base address is simply zero:
    ///
    ///  - ELF images of dynamic libraries (i.e. not executables) usually have a
    ///    base address of zero.
    ///  - Stand-alone mach-O dylibs usually have a base address of zero because their
    ///    __TEXT segment is at address zero.
    ///  - In PDBs, "RVAs" are relative addresses which are already relative to the
    ///    image base.
    ///
    /// However, in the following cases, the base address is usually non-zero:
    ///
    ///  - The "image base address" of Windows binaries is usually non-zero.
    ///  - mach-O executable files (not dylibs) usually have their __TEXT segment at
    ///    address 0x100000000.
    ///  - mach-O libraries in the dyld shared cache have a __TEXT segment at some
    ///    non-zero address in the cache.
    ///  - ELF executables can have non-zero base addresses, e.g. 0x200000 or 0x400000.
    ///  - Kernel ELF binaries ("vmlinux") have a large base address such as
    ///    0xffffffff81000000. Moreover, the base address seems to coincide with the
    ///    vmaddr of the .text section, which is readily-available in perf.data files
    ///    (in a synthetic mapping called "[kernel.kallsyms]_text").
    fn samply_relative_address_base(&self) -> u64;
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

    fn samply_relative_address_base(&self) -> u64 {
        use object::read::ObjectSegment;
        if let Some(text_segment) = self.segments().find(|s| s.name() == Ok(Some("__TEXT"))) {
            // This is a mach-O image. "Relative addresses" are relative to the
            // vmaddr of the __TEXT segment.
            return text_segment.address();
        }

        if let FileFlags::Elf { .. } = self.flags() {
            // This is an ELF image. "Relative addresses" are relative to the
            // vmaddr of the first segment (the first LOAD command).
            if let Some(first_segment) = self.segments().next() {
                return first_segment.address();
            }
        }

        // For PE binaries, relative_address_base() returns the image base address.
        self.relative_address_base()
    }
}
