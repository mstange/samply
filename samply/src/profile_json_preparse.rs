use std::ffi::OsString;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::{collections::HashMap, str::FromStr};

use debugid::DebugId;
use flate2::bufread::GzDecoder;
use serde_derive::Deserialize;
use wholesym::{CodeId, LibraryInfo};

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonProcess {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
    #[serde(default)]
    pub threads: Vec<ProfileJsonThread>,
    #[serde(default)]
    pub processes: Vec<ProfileJsonProcess>,
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonThread {
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
    // Read the profile.json file and parse it as JSON.
    // Build a map (debugName, breakpadID) -> debugPath from the information
    // in profile(\.processes\[\d+\])*(\.threads\[\d+\])?\.libs.
    let reader = BufReader::new(file);

    // Handle .gz profiles
    if filename.extension() == Some(&OsString::from("gz")) {
        let decoder = GzDecoder::new(reader);
        let reader = BufReader::new(decoder);
        parse_libinfo_map_from_profile(reader)
    } else {
        parse_libinfo_map_from_profile(reader)
    }
}

fn parse_libinfo_map_from_profile(
    reader: impl std::io::Read,
) -> Result<HashMap<(String, DebugId), LibraryInfo>, std::io::Error> {
    let profile: ProfileJsonProcess = serde_json::from_reader(reader)?;
    let mut libinfo_map = HashMap::new();
    add_to_libinfo_map_recursive(&profile, &mut libinfo_map);
    Ok(libinfo_map)
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
    profile: &ProfileJsonProcess,
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
        let p: ProfileJsonProcess = serde_json::from_str("{}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonProcess = serde_json::from_str("{\"unknown_field\":[1, 2, 3]}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonProcess =
            serde_json::from_str("{\"threads\":[{\"libs\":[{}]}]}").unwrap();
        assert!(p.libs.is_empty());
        assert_eq!(p.threads.len(), 1);
        assert_eq!(p.threads[0].libs.len(), 1);
        assert_eq!(p.threads[0].libs[0], ProfileJsonLib::default());
        assert!(p.processes.is_empty());
    }
}
