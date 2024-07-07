use std::path::{Path, PathBuf};

use debugid::CodeId;
use fxprof_processed_profile::{LibraryHandle, LibraryInfo, Profile};
use linux_perf_data::jitdump::JitDumpHeader;
use wholesym::samply_symbols::debug_id_and_code_id_for_jitdump;

pub fn open_file_with_fallback<P: AsRef<Path>>(
    path: &Path,
    extra_dirs: &[P],
) -> std::io::Result<(std::fs::File, PathBuf)> {
    let e = match std::fs::File::open(path) {
        Ok(file) => return Ok((file, path.to_owned())),
        Err(e) => e,
    };

    if let Some(filename) = path.file_name() {
        for dir in extra_dirs {
            let p: PathBuf = [dir.as_ref(), Path::new(filename)].iter().collect();
            if let Ok(file) = std::fs::File::open(&p) {
                return Ok((file, p));
            }
        }
    }

    Err(e)
}

pub fn lib_handle_for_jitdump(
    path: &Path,
    header: &JitDumpHeader,
    profile: &mut Profile,
) -> LibraryHandle {
    let (debug_id, code_id_bytes) =
        debug_id_and_code_id_for_jitdump(header.pid, header.timestamp, header.elf_machine_arch);
    let code_id = CodeId::from_binary(&code_id_bytes);
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let path = path.to_string_lossy().into_owned();

    profile.add_lib(LibraryInfo {
        debug_name: name.clone(),
        debug_path: path.clone(),
        name,
        path,
        debug_id,
        code_id: Some(code_id.to_string()),
        arch: None,
        symbol_table: None,
    })
}
