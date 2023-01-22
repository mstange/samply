use nom::branch::alt;
use nom::bytes::complete::{tag, take_till1, take_until1, take_while1};
use nom::character::complete::one_of;
use nom::error::ErrorKind;
use nom::sequence::{delimited, preceded, terminated, tuple};
use nom::Err;

use crate::MappedPath;

use std::collections::HashMap;

pub trait ExtraPathMapper {
    fn map_path(&mut self, path: &str) -> Option<MappedPath>;
}

impl ExtraPathMapper for () {
    fn map_path(&mut self, _path: &str) -> Option<MappedPath> {
        None
    }
}

pub struct PathMapper<E: ExtraPathMapper> {
    cache: HashMap<String, Option<MappedPath>>,
    extra_mapper: Option<E>,
}

impl<E: ExtraPathMapper> PathMapper<E> {
    pub fn new() -> Self {
        Self::new_with_maybe_extra_mapper(None)
    }

    pub fn new_with_maybe_extra_mapper(extra_mapper: Option<E>) -> Self {
        PathMapper {
            cache: HashMap::new(),
            extra_mapper,
        }
    }

    /// Compute the mapped path for a raw path.
    pub fn map_path(&mut self, raw_path: &str) -> Option<MappedPath> {
        if let Some(extra_mapper) = &mut self.extra_mapper {
            if let Some(mapped_path) = extra_mapper.map_path(raw_path) {
                return Some(mapped_path);
            }
        }

        if let Some(value) = self.cache.get(raw_path) {
            return value.clone();
        }

        let mapped_path = if let Ok(mapped_path) = map_rustc_path(raw_path) {
            Some(mapped_path)
        } else if let Ok(mapped_path) = map_cargo_dep_path(raw_path) {
            Some(mapped_path)
        } else {
            None
        };
        self.cache.insert(raw_path.into(), mapped_path.clone());
        mapped_path
    }
}

fn map_rustc_path(input: &str) -> Result<MappedPath, nom::Err<nom::error::Error<&str>>> {
    // /rustc/c79419af0721c614d050f09b95f076da09d37b0d/library/std/src/rt.rs
    // /rustc/e1884a8e3c3e813aada8254edfa120e85bf5ffca\/library\std\src\rt.rs
    // /rustc/a178d0322ce20e33eac124758e837cbd80a6f633\library\std\src\rt.rs
    let (input, rev) = delimited(
        tag("/rustc/"),
        take_till1(|c| c == '/' || c == '\\'),
        take_while1(|c| c == '/' || c == '\\'),
    )(input)?;
    let path = input.replace('\\', "/");
    Ok(MappedPath::Git {
        repo: "github.com/rust-lang/rust".into(),
        path,
        rev: rev.to_owned(),
    })
}

fn map_cargo_dep_path(input: &str) -> Result<MappedPath, nom::Err<nom::error::Error<&str>>> {
    // /Users/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/nom-7.1.3/src/bytes/complete.rs
    let (input, (registry, crate_name_and_version)) = preceded(
        tuple((
            alt((take_until1("/.cargo"), take_until1("\\.cargo"))),
            delimited(one_of("/\\"), tag(".cargo"), one_of("/\\")),
            terminated(tag("registry"), one_of("/\\")),
            terminated(tag("src"), one_of("/\\")),
        )),
        tuple((
            terminated(take_till1(|c| c == '/' || c == '\\'), one_of("/\\")),
            terminated(take_till1(|c| c == '/' || c == '\\'), one_of("/\\")),
        )),
    )(input)?;
    let (crate_name, version) = match crate_name_and_version.rfind('-') {
        Some(pos) => (
            &crate_name_and_version[..pos],
            &crate_name_and_version[(pos + 1)..],
        ),
        None => {
            return Err(Err::Error(nom::error::Error::new(
                crate_name_and_version,
                ErrorKind::Digit,
            )))
        }
    };
    let path = input.replace('\\', "/");
    Ok(MappedPath::Cargo {
        registry: registry.to_owned(),
        crate_name: crate_name.to_owned(),
        version: version.to_owned(),
        path,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_map_rustc_path() {
        assert_eq!(
            map_rustc_path(
                r#"/rustc/c79419af0721c614d050f09b95f076da09d37b0d/library/std/src/rt.rs"#
            ),
            Ok(MappedPath::Git {
                repo: "github.com/rust-lang/rust".into(),
                path: "library/std/src/rt.rs".into(),
                rev: "c79419af0721c614d050f09b95f076da09d37b0d".into()
            })
        );
        assert_eq!(
            map_rustc_path(
                r#"/rustc/e1884a8e3c3e813aada8254edfa120e85bf5ffca\/library\std\src\rt.rs"#
            ),
            Ok(MappedPath::Git {
                repo: "github.com/rust-lang/rust".into(),
                path: "library/std/src/rt.rs".into(),
                rev: "e1884a8e3c3e813aada8254edfa120e85bf5ffca".into()
            })
        );
        assert_eq!(
            map_rustc_path(
                r#"/rustc/a178d0322ce20e33eac124758e837cbd80a6f633\library\std\src\rt.rs"#
            ),
            Ok(MappedPath::Git {
                repo: "github.com/rust-lang/rust".into(),
                path: "library/std/src/rt.rs".into(),
                rev: "a178d0322ce20e33eac124758e837cbd80a6f633".into()
            })
        );
    }

    #[test]
    fn test_map_cargo_dep_path() {
        assert_eq!(
            map_cargo_dep_path(
                r#"/Users/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/nom-7.1.3/src/bytes/complete.rs"#
            ),
            Ok(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".into(),
                crate_name: "nom".into(),
                version: "7.1.3".into(),
                path: "src/bytes/complete.rs".into(),
            })
        );
    }
}
