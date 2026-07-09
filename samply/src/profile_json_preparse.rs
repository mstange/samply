use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read};
use std::path::Path;
use std::str::FromStr;

use debugid::DebugId;
use flate2::bufread::GzDecoder;
use json_slabs::{RootJsonReader, MAGIC as JSLB_MAGIC};
use serde_derive::Deserialize;
use wholesym::{CodeId, LibraryInfo};

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonNode {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
    #[serde(default)]
    pub threads: Vec<ProfileJsonThread>,
    #[serde(default)]
    pub processes: Vec<ProfileJsonNode>,
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonThread {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
}

/// Minimal skeleton for a JSLB root profile. JSLB is always a "processed"
/// profile of format version >= 65, which stores all library info at
/// `profile.libs`, so we only inspect that field. Other fields such as
/// `threads` or `shared` contain slab placeholders like `{"$s":N}` rather
/// than sequences, and serde skips them as unknown fields.
#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonRootLibs {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonLib {
    pub debug_name: Option<String>,
    pub debug_path: Option<String>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub breakpad_id: Option<String>,
    pub code_id: Option<String>,
    pub arch: Option<String>,
}

pub fn parse_libinfo_map_from_profile_file(
    file: File,
    filename: &Path,
) -> Result<HashMap<(String, DebugId), LibraryInfo>, std::io::Error> {
    // Read the profile file and build a map (debugName, breakpadID) -> debugPath.
    // Supported inputs (any of which may additionally be gzipped):
    //   * "Gecko" JSON profile — libs at `profile.libs` and per subprocess
    //     under `profile.processes[i].libs`.
    //   * "processed" JSON profile of format version < 41 — libs at
    //     `profile.threads[i].libs`.
    //   * "processed" JSON profile of format version >= 41 — libs at
    //     `profile.libs`. See
    //     https://github.com/firefox-devtools/profiler/blob/e7f99034daccd4b069cb2d309d9541e80d5a4da5/docs-developer/CHANGELOG-formats.md#version-41
    //   * JSLB (JsonSlabs) container — always a processed profile of format
    //     version >= 65, so libs are always at `profile.libs`.
    let reader = BufReader::new(file);
    if filename.extension() == Some(&OsString::from("gz")) {
        parse_libinfo_map_from_profile(GzDecoder::new(reader))
    } else {
        parse_libinfo_map_from_profile(reader)
    }
}

fn parse_libinfo_map_from_profile<R: Read>(
    mut reader: R,
) -> Result<HashMap<(String, DebugId), LibraryInfo>, std::io::Error> {
    // Peek at the first few bytes to distinguish a JSLB container from raw
    // JSON, then hand the reader (with the peeked bytes chained back in
    // front) to serde_json.
    let mut magic_buf = [0u8; JSLB_MAGIC.len()];
    let n = read_up_to(&mut reader, &mut magic_buf)?;
    let is_jslb = n == JSLB_MAGIC.len() && magic_buf == JSLB_MAGIC;
    let head = Cursor::new(magic_buf).take(n as u64);
    let combined = head.chain(reader);

    let mut libinfo_map = HashMap::new();
    if is_jslb {
        // JSLB is always a processed profile of format version >= 65, which
        // stores all library info at `profile.libs`. Per-thread `libs` only
        // appear in processed profiles below format version 41, which predate
        // JSLB and are only ever plain JSON — so we don't need to descend
        // into `threads` or `processes` here.
        // Buffer *around* RootJsonReader so that its `remaining` cap prevents
        // the buffered layer from prefetching past the root slab.
        let profile: ProfileJsonRootLibs =
            serde_json::from_reader(BufReader::new(RootJsonReader::new(combined)?))?;
        add_libs_to_libinfo_map(&profile.libs, &mut libinfo_map);
    } else {
        let profile: ProfileJsonNode = serde_json::from_reader(combined)?;
        add_to_libinfo_map_recursive(&profile, &mut libinfo_map);
    }
    Ok(libinfo_map)
}

fn read_up_to<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let m = reader.read(&mut buf[n..])?;
        if m == 0 {
            break;
        }
        n += m;
    }
    Ok(n)
}

fn add_libs_to_libinfo_map(
    libs: &[ProfileJsonLib],
    libinfo_map: &mut HashMap<(String, DebugId), LibraryInfo>,
) {
    for lib in libs {
        if let Some(lib_info) = libinfo_map_entry_for_lib(lib) {
            // If libinfo_map_entry_for_lib returns Some(), debug_name and debug_id are guaranteed to be Some().
            let debug_name = lib_info.debug_name.clone().unwrap();
            let debug_id = lib_info.debug_id.unwrap();
            libinfo_map.insert((debug_name, debug_id), lib_info);
        }
    }
}

fn libinfo_map_entry_for_lib(lib: &ProfileJsonLib) -> Option<LibraryInfo> {
    let debug_name = lib.debug_name.clone()?;
    let breakpad_id = lib.breakpad_id.as_ref()?;
    let debug_path = lib.debug_path.clone();
    let name = lib.name.clone();
    let path = lib.path.clone();
    let debug_id = DebugId::from_breakpad(breakpad_id).ok()?;
    let code_id = lib
        .code_id
        .as_deref()
        .and_then(|ci| CodeId::from_str(ci).ok());
    let arch = lib.arch.clone();
    let lib_info = LibraryInfo {
        debug_id: Some(debug_id),
        debug_name: Some(debug_name),
        debug_path,
        name,
        code_id,
        path,
        arch,
    };
    Some(lib_info)
}

fn add_to_libinfo_map_recursive(
    profile: &ProfileJsonNode,
    libinfo_map: &mut HashMap<(String, DebugId), LibraryInfo>,
) {
    add_libs_to_libinfo_map(&profile.libs, libinfo_map);
    for thread in &profile.threads {
        add_libs_to_libinfo_map(&thread.libs, libinfo_map);
    }
    for process in &profile.processes {
        add_to_libinfo_map_recursive(process, libinfo_map);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn deserialize_profile_json() {
        let p: ProfileJsonNode = serde_json::from_str("{}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonNode = serde_json::from_str("{\"unknown_field\":[1, 2, 3]}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonNode = serde_json::from_str("{\"threads\":[{\"libs\":[{}]}]}").unwrap();
        assert!(p.libs.is_empty());
        assert_eq!(p.threads.len(), 1);
        assert_eq!(p.threads[0].libs.len(), 1);
        assert_eq!(p.threads[0].libs[0], ProfileJsonLib::default());
        assert!(p.processes.is_empty());
    }
}
