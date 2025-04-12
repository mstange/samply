use std::sync::OnceLock;

use object::Object;
use wholesym::{CodeId, ElfBuildId};

/// Returns the memory address range in this process where the VDSO is mapped.
pub fn get_vdso_range() -> Option<(usize, usize)> {
    let proc_maps_file = std::fs::File::open("/proc/self/maps").ok()?;
    use std::io::{BufRead, BufReader};
    let mut lines = BufReader::new(proc_maps_file).lines().map_while(Result::ok);
    // "ffffa613c000-ffffa613d000 r-xp 00000000 00:00 0                          [vdso]"
    let proc_maps_vdso_line = lines.find(|l| l.ends_with("[vdso]"))?;
    let (start, end) = proc_maps_vdso_line.split_once(' ')?.0.split_once('-')?;
    Some((
        usize::from_str_radix(start, 16).ok()?,
        usize::from_str_radix(end, 16).ok()?,
    ))
}

/// Returns the in-memory slice that contains the VDSO, if found.
pub fn get_vdso_data() -> Option<&'static [u8]> {
    let (start, end) = get_vdso_range()?;
    let len = end.checked_sub(start)?;
    // Make a slice around the vdso contents.
    //
    // Safety: the address range came from /proc/self/maps and the VDSO mapping
    // does not change throughout the lifetime of the process. It contains immutable
    // and initial data.
    Some(unsafe { core::slice::from_raw_parts(start as *const u8, len) })
}

pub struct VdsoObject {
    object: object::File<'static, &'static [u8]>,
    build_id: &'static [u8],
    code_id: CodeId,
}

impl VdsoObject {
    pub fn shared_instance_for_this_process() -> Option<&'static Self> {
        static INSTANCE: OnceLock<Option<VdsoObject>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| {
                let data = get_vdso_data()?;
                // Parse the data as an ELF file.
                // This works more or less by accident; object's parsing is made for
                // objects stored on disk, not for objects loaded into memory.
                // However, the VDSO in-memory image happens to be similar enough to its
                // equivalent on-disk image that this works fine. Most importantly, the
                // VDSO's section SVMAs match the section file offsets.
                let object = object::File::parse(data).ok()?;
                let build_id = object.build_id().ok()??;
                let code_id = CodeId::ElfBuildId(ElfBuildId::from_bytes(build_id));
                Some(VdsoObject {
                    object,
                    build_id,
                    code_id,
                })
            })
            .as_ref()
    }

    pub fn object(&self) -> &object::File<'static, &'static [u8]> {
        &self.object
    }

    pub fn code_id(&self) -> &CodeId {
        &self.code_id
    }

    pub fn build_id(&self) -> &[u8] {
        self.build_id
    }
}
