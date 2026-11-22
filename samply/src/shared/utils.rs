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
    })
}

/// Simple, reasonably fast matching against a preprocessed glob-like pattern.
///
/// pattern needs to contain at least 2 elements.
/// pattern[0] is a fixed prefix, matching from the start of the haystack. It should be len()==0 if the haystack isn't required to start with it.
/// pattern[pattern.len()-1] the same applies as for the prefix, but this is the suffix of course.
/// pattern[1..pattern.len()-1] should not include any 0-length segments. (panics)
///
/// valid inputs:
/// &[b"foo", b""] -> matches `foo`, `foobar`, doesn't match `barfoo`, `barfoobaz`
/// &[b"", b"foo"] -> matches `foo`, `barfoo`, doesn't match `foobaz`, `barfoobaz`
/// &[b"", b"foo", b""] -> matches `foo`, `foobar`, `barfoo`, doesn't match: `baz`
///
/// Note: This is structured such that a pattern can be parsed from a string easily, using, for example: `"*foo*bar".split('*').map(|pat| pat.as_bytes().to_vec()).collect()`
pub fn glob_like_match<T: AsRef<[u8]>>(
    pattern: &impl AsRef<[T]>,
    haystack: &impl AsRef<[u8]>,
) -> bool {
    let pattern = pattern.as_ref();
    assert!(pattern.len() >= 2);
    let mut rest = haystack.as_ref();
    let (prefix, pattern) = pattern.split_first().unwrap(); // Safety: assert at the start requires min 2 elements
    let prefix = prefix.as_ref();
    let (suffix, pattern) = pattern.split_last().unwrap();
    let suffix = suffix.as_ref();

    if !rest.starts_with(prefix) {
        return false;
    }
    rest = &rest[prefix.len()..];

    if !rest.ends_with(suffix) {
        return false;
    }
    rest = &rest[..rest.len() - suffix.len()];

    for seg in pattern {
        let seg = seg.as_ref();
        let pos = rest.windows(seg.len()).position(|w| w == seg);
        match pos {
            Some(p) => rest = &rest[p + seg.len()..],
            None => return false,
        }
    }

    true
}
#[cfg(test)]
mod tests {
    use crate::shared::utils::glob_like_match;

    #[test]
    fn test_glob_like_match() {
        let pattern = &[b"foo".as_ref(), b"".as_ref()];
        assert!(glob_like_match(pattern, b"foo"));
        assert!(glob_like_match(pattern, b"foobar"));
        assert!(!glob_like_match(pattern, b"barfoo"));
        assert!(!glob_like_match(pattern, b"barfoobaz"));
        assert!(!glob_like_match(pattern, b"barbaz"));

        let pattern = &[b"".as_ref(), b"foo".as_ref()];
        assert!(glob_like_match(pattern, b"foo"));
        assert!(glob_like_match(pattern, b"barfoo"));
        assert!(!glob_like_match(pattern, b"foobar"));
        assert!(!glob_like_match(pattern, b"barfoobaz"));
        assert!(!glob_like_match(pattern, b"barbaz"));

        let pattern = &[b"".as_ref(), b"foo".as_ref(), b"".as_ref()];
        assert!(glob_like_match(pattern, b"foo"));
        assert!(glob_like_match(pattern, b"barfoo"));
        assert!(glob_like_match(pattern, b"foobar"));
        assert!(glob_like_match(pattern, b"barfoobaz"));
        assert!(!glob_like_match(pattern, b"barbaz"));
    }
}
