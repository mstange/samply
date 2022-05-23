use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::shared::{BasePath, FilePath};

pub trait ExtraPathMapper {
    fn map_path(&mut self, path: &str) -> Option<String>;
}

impl ExtraPathMapper for () {
    fn map_path(&mut self, _path: &str) -> Option<String> {
        None
    }
}

pub struct PathMapper<E: ExtraPathMapper> {
    base_path: BasePath,
    cache: HashMap<String, FilePath>,
    extra_mapper: Option<E>,
    rustc_regex: Regex,
    cargo_dep_regex: Regex,
}

impl<'a, E: ExtraPathMapper> PathMapper<E> {
    pub fn new(base_path: &BasePath) -> Self {
        Self::new_with_maybe_extra_mapper(base_path, None)
    }

    pub fn new_with_maybe_extra_mapper(base_path: &BasePath, extra_mapper: Option<E>) -> Self {
        PathMapper {
            base_path: base_path.clone(),
            cache: HashMap::new(),
            extra_mapper,
            rustc_regex: Regex::new(r"^/rustc/(?P<rev>[0-9a-f]+)\\?[/\\](?P<path>.*)$").unwrap(),
            cargo_dep_regex: Regex::new(r"[/\\]\.cargo[/\\]registry[/\\]src[/\\](?P<registry>[^/\\]+)[/\\](?P<crate>[^/]+)-(?P<version>[0-9]+\.[0-9]+\.[0-9]+)[/\\](?P<path>.*)$").unwrap(),
        }
    }

    /// Map the raw path to a `FilePath`.
    ///
    /// If `self.base_path` is `BasePath::CanReferToLocalFiles`, raw_path can be
    /// a relative or an absolute path on the local machine which is resolved with
    /// respect to `self.base_path`.
    pub fn map_path(&mut self, raw_path: &str) -> FilePath {
        if let Some(extra_mapper) = &mut self.extra_mapper {
            if let Some(mapped_path) = extra_mapper.map_path(raw_path) {
                let file_path = match &self.base_path {
                    BasePath::NoLocalSourceFileAccess => FilePath::NonLocal(mapped_path),
                    BasePath::CanReferToLocalFiles(base) => FilePath::LocalMapped {
                        local: make_abs_path(base, raw_path),
                        mapped: mapped_path,
                    },
                };
                return file_path;
            }
        }

        if let Some(value) = self.cache.get(raw_path) {
            return value.clone();
        }

        let mapped_path = if let Some(captures) = self.rustc_regex.captures(raw_path) {
            let rev = captures.name("rev").unwrap().as_str();
            let path = captures.name("path").unwrap().as_str();
            let path = path.replace('\\', "/");
            Some(format!("git:github.com/rust-lang/rust:{}:{}", path, rev))
        } else if let Some(captures) = self.cargo_dep_regex.captures(raw_path) {
            let registry = captures.name("registry").unwrap().as_str();
            let crate_ = captures.name("crate").unwrap().as_str();
            let version = captures.name("version").unwrap().as_str();
            let path = captures.name("path").unwrap().as_str();
            let path = path.replace('\\', "/");
            Some(format!(
                "cargo:{}:{}-{}:{}",
                registry, crate_, version, path
            ))
        } else {
            None
        };

        let file_path = if let BasePath::CanReferToLocalFiles(base) = &self.base_path {
            let rel_or_abs = Path::new(raw_path);
            if rel_or_abs.is_absolute() {
                // raw_path is an absolute path, referring to a file on this machine.
                let local = rel_or_abs.to_owned();
                match mapped_path {
                    Some(mapped) => FilePath::LocalMapped { local, mapped },
                    None => FilePath::Local(local),
                }
            } else {
                // raw_path is a relative path. Treat it as a "mapped" path, unless
                // we already have some other mapped path.
                let local = base.join(rel_or_abs);
                let mapped = mapped_path.unwrap_or_else(|| raw_path.to_owned());
                FilePath::LocalMapped { local, mapped }
            }
        } else {
            FilePath::NonLocal(mapped_path.unwrap_or_else(|| raw_path.to_owned()))
        };

        self.cache.insert(raw_path.into(), file_path.clone());
        file_path
    }
}

fn make_abs_path(base: &Path, rel_or_abs: &str) -> PathBuf {
    let rel_or_abs = Path::new(rel_or_abs);
    if rel_or_abs.is_absolute() {
        rel_or_abs.to_owned()
    } else {
        base.join(rel_or_abs)
    }
}
