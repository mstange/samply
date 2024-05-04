// This code was taken from https://github.com/gimli-rs/moria/ , which is currently
// not released on crates.io.

#![warn(clippy::all)]

use std::fs;
use std::path::{Path, PathBuf};

use object::Object;
use samply_symbols::object;
use uuid::Uuid;

#[cfg(target_os = "macos")]
pub use crate::moria_mac_spotlight::locate_dsym_using_spotlight;

#[cfg(not(target_os = "macos"))]
pub fn locate_dsym_using_spotlight(_uuid: uuid::Uuid) -> Result<PathBuf, &'static str> {
    Err("Could not locate dSYM")
}

/// On macOS it can take some time for spotlight to index the dSYM file and on other OSes it is
/// impossible to use spotlight. When built by cargo, we can likely find the dSYM file in
/// target/<profile>/deps or target/<profile>/examples. Otherwise it can likely be found at
/// <filename>.dSYM. This function will try to find it there.
///
/// # Arguments
///
/// * Parsed version of the object file which needs its debuginfo.
/// * Path to the object file.
pub fn locate_dsym_fastpath(path: &Path, uuid: Uuid) -> Option<PathBuf> {
    // Canonicalize the path to make sure the fastpath also works when current working
    // dir is inside target/
    let path = path.canonicalize().ok()?;

    // First try <path>.dSYM
    let mut dsym = path.file_name()?.to_owned();
    dsym.push(".dSYM");
    let dsym_dir = path.with_file_name(&dsym);
    if let Some(f) = try_match_dsym(&dsym_dir, uuid) {
        return Some(f);
    }

    // Get the path to the target dir of the current build channel.
    let mut target_channel_dir = &*path;
    loop {
        let parent = target_channel_dir.parent()?;
        target_channel_dir = parent;

        if target_channel_dir.parent().and_then(Path::file_name)
            == Some(std::ffi::OsStr::new("target"))
        {
            break; // target_dir = ???/target/<channel>
        }
    }

    // Check every entry in <target_channel_dir>/deps and <target_channel_dir>/examples
    let deps_dir = target_channel_dir.join("deps");
    let examples_dir = target_channel_dir.join("examples");
    try_match_dsym_in_dir(&deps_dir, uuid).or_else(|| try_match_dsym_in_dir(&examples_dir, uuid))
}

fn try_match_dsym_in_dir(dir: &Path, uuid: Uuid) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let item = entry.ok()?.path();

        // If not a dSYM dir, try next entry.
        if item.extension() != Some(std::ffi::OsStr::new("dSYM")) {
            continue;
        }

        if let Some(debug_file_name) = try_match_dsym(&item, uuid) {
            return Some(debug_file_name);
        }
    }

    None
}

fn try_match_dsym(dsym_dir: &Path, uuid: Uuid) -> Option<PathBuf> {
    // Get path to inner object file.
    let mut dir_iter = fs::read_dir(dsym_dir.join("Contents/Resources/DWARF")).ok()?;

    let debug_file_name = dir_iter.next()?.ok()?.path();

    if dir_iter.next().is_some() {
        return None; // There should only be one file in the `DWARF` directory.
    }

    // Parse inner object file.
    let file = fs::read(&debug_file_name).ok()?;
    let dsym = object::File::parse(&file[..]).ok()?;

    // Make sure the dSYM file matches the object file to find debuginfo for.
    if dsym.mach_uuid() == Ok(Some(*uuid.as_bytes())) {
        Some(debug_file_name)
    } else {
        None
    }
}
