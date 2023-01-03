use regex::Regex;
use std::collections::HashMap;

pub trait ExtraPathMapper {
    fn map_path(&mut self, path: &str) -> Option<String>;
}

impl ExtraPathMapper for () {
    fn map_path(&mut self, _path: &str) -> Option<String> {
        None
    }
}

pub struct PathMapper<E: ExtraPathMapper> {
    cache: HashMap<String, Option<String>>,
    extra_mapper: Option<E>,
    rustc_regex: Regex,
    cargo_dep_regex: Regex,
}

impl<E: ExtraPathMapper> PathMapper<E> {
    pub fn new() -> Self {
        Self::new_with_maybe_extra_mapper(None)
    }

    pub fn new_with_maybe_extra_mapper(extra_mapper: Option<E>) -> Self {
        PathMapper {
            cache: HashMap::new(),
            extra_mapper,
            rustc_regex: Regex::new(r"^/rustc/(?P<rev>[0-9a-f]+)\\?[/\\](?P<path>.*)$").unwrap(),
            cargo_dep_regex: Regex::new(r"[/\\]\.cargo[/\\]registry[/\\]src[/\\](?P<registry>[^/\\]+)[/\\](?P<crate>[^/]+)-(?P<version>[0-9]+\.[0-9]+\.[0-9]+)[/\\](?P<path>.*)$").unwrap(),
        }
    }

    /// Compute the mapped path for a raw path.
    pub fn map_path(&mut self, raw_path: &str) -> Option<String> {
        if let Some(extra_mapper) = &mut self.extra_mapper {
            if let Some(mapped_path) = extra_mapper.map_path(raw_path) {
                return Some(mapped_path);
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
        self.cache.insert(raw_path.into(), mapped_path.clone());
        mapped_path
    }
}
