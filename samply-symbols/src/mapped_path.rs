use std::borrow::Cow;

use nom::branch::alt;
use nom::bytes::complete::{tag, take_till1, take_until1, take_while1};
use nom::character::complete::one_of;
use nom::error::ErrorKind;
use nom::sequence::{delimited, preceded, terminated, tuple};
use nom::Err;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum UnparsedMappedPath {
    Url(String),
    BreakpadSpecialPath(String),
    RawPath(String),
}

impl UnparsedMappedPath {
    pub fn from_special_path_str(breakpad_special_path: &str) -> Self {
        Self::BreakpadSpecialPath(breakpad_special_path.to_owned())
    }
    pub fn from_url(url: &str) -> Self {
        Self::Url(url.to_owned())
    }
    pub fn from_raw_path(raw_path: &str) -> Self {
        Self::RawPath(raw_path.to_owned())
    }
    pub fn parse(&self) -> Option<MappedPath> {
        match self {
            UnparsedMappedPath::Url(url) => MappedPath::from_url(url),
            UnparsedMappedPath::BreakpadSpecialPath(bsp) => MappedPath::from_special_path_str(bsp),
            UnparsedMappedPath::RawPath(raw) => MappedPath::from_raw_path(raw),
        }
    }
    pub fn display_path(&self) -> Option<Cow<'_, str>> {
        if let Some(mp) = self.parse() {
            let display_path = mp.display_path().into_owned();
            Some(display_path.into())
        } else {
            None
        }
    }
    pub fn special_path_str(&self) -> Option<Cow<'_, str>> {
        let mp = match self {
            UnparsedMappedPath::Url(url) => MappedPath::from_url(url),
            UnparsedMappedPath::BreakpadSpecialPath(bsp) => return Some(bsp.into()),
            UnparsedMappedPath::RawPath(raw) => MappedPath::from_raw_path(raw),
        };
        if let Some(mp) = mp {
            let display_path = mp.to_special_path_str();
            Some(display_path.into())
        } else {
            None
        }
    }
}

/// A special source file path for source files which are hosted online.
///
/// About "special path" strings: Special paths strings are a string serialization
/// of a mapped path. The format of this string was adopted from the format used
/// in [Firefox Breakpad .sym files](https://searchfox.org/mozilla-central/rev/4ebfb48f7e82251145afa4a822f970931dd06c68/toolkit/crashreporter/tools/symbolstore.py#199).
/// This format is also used in the [Tecken symbolication API](https://tecken.readthedocs.io/en/latest/symbolication.html#id9),
/// and [internally in the Firefox Profiler](https://github.com/firefox-devtools/profiler/blob/fb9ff03ab8b98d9e7c29e36314080fae555bbe78/src/utils/special-paths.js).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MappedPath {
    /// A path to a file in a git repository.
    Git {
        /// The web host + root path where the repository is hosted, e.g. `"github.com/rust-lang/rust"`.
        repo: String,
        /// The path to this file inside the repository, e.g. `"library/std/src/sys/unix/thread.rs"`.
        path: String,
        /// The revision, e.g. `"53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b"`.
        rev: String,
    },
    /// A path to a file in a mercurial repository (hg).
    Hg {
        /// The web host + root path where the repository is hosted, e.g. `"hg.mozilla.org/mozilla-central"`.
        repo: String,
        /// The path to this file inside the repository, e.g. `"widget/cocoa/nsAppShell.mm"`.
        path: String,
        /// The revision, e.g. `"997f00815e6bc28806b75448c8829f0259d2cb28"`.
        rev: String,
    },
    /// A path to a file hosted in an S3 bucket.
    S3 {
        /// The name of the S3 bucket, e.g. `"gecko-generated-sources"` (which is hosted at `https://gecko-generated-sources.s3.amazonaws.com/`).
        bucket: String,
        /// The "digest" of the file, i.e. a long hash string of hex characters.
        digest: String,
        /// The path to this file inside the bucket, e.g. `"ipc/ipdl/PBackgroundChild.cpp"`.
        path: String,
    },
    /// A path to a file in a Rust package which is hosted in a cargo registry (usually on crates.io).
    Cargo {
        /// The name of the cargo registry, usually `"github.com-1ecc6299db9ec823"`.
        registry: String,
        /// The name of the package, e.g. `"tokio"`.
        crate_name: String,
        /// The version of the package, e.g. `"1.6.1"`.
        version: String,
        /// The path to this file inside the package, e.g. `"src/runtime/task/mod.rs"`.
        path: String,
    },
}

impl MappedPath {
    /// Parse a "special path" string. These types of strings are found in Breakpad
    /// .sym files on the Mozilla symbol server.
    ///
    /// So this parsing code basically exists here because this crate supports obtaining
    /// symbols from Breakpad symbol files, so that consumers don't have parse this
    /// syntax when looking up symbols from a `SymbolMap` from such a .sym file.
    pub fn from_special_path_str(special_path: &str) -> Option<Self> {
        parse_special_path(special_path)
    }

    /// Detect some URLs of plain text files and convert them to a `MappedPath`.
    pub fn from_url(url: &str) -> Option<Self> {
        parse_url(url)
    }

    pub fn from_raw_path(raw_path: &str) -> Option<Self> {
        parse_raw_path(raw_path)
    }

    /// Serialize this mapped path to a string, using the "special path" syntax.
    pub fn to_special_path_str(&self) -> String {
        match self {
            MappedPath::Git { repo, path, rev } => format!("git:{repo}:{path}:{rev}"),
            MappedPath::Hg { repo, path, rev } => format!("hg:{repo}:{path}:{rev}"),
            MappedPath::S3 {
                bucket,
                digest,
                path,
            } => format!("s3:{bucket}:{digest}/{path}:"),
            MappedPath::Cargo {
                registry,
                crate_name,
                version,
                path,
            } => format!("cargo:{registry}:{crate_name}-{version}:{path}"),
        }
    }

    /// Create a short, display-friendly form of this path.
    pub fn display_path(&self) -> Cow<'_, str> {
        match self {
            MappedPath::Git { path, .. } => path.into(),
            MappedPath::Hg { path, .. } => path.into(),
            MappedPath::S3 { path, .. } => path.into(),
            MappedPath::Cargo {
                crate_name,
                version,
                path,
                ..
            } => format!("{crate_name}-{version}/{path}").into(),
        }
    }
}

fn git_path(input: &str) -> Option<(String, String, String)> {
    let input = input.strip_prefix("git:")?;
    let (repo, input) = input.split_once(':')?;
    let (path, rev) = input.split_once(':')?;
    Some((repo.to_owned(), path.to_owned(), rev.to_owned()))
}

fn hg_path(input: &str) -> Option<(String, String, String)> {
    let input = input.strip_prefix("hg:")?;
    let (repo, input) = input.split_once(':')?;
    let (path, rev) = input.split_once(':')?;
    Some((repo.to_owned(), path.to_owned(), rev.to_owned()))
}

fn s3_path(input: &str) -> Option<(String, String, String)> {
    let input = input.strip_prefix("s3:")?;
    let (bucket, input) = input.split_once(':')?;
    let (digest, input) = input.split_once('/')?;
    let path = input.strip_suffix(':')?;
    Some((bucket.to_owned(), digest.to_owned(), path.to_owned()))
}

fn cargo_path(input: &str) -> Option<(String, String, String, String)> {
    let input = input.strip_prefix("cargo:")?;
    let (registry, input) = input.split_once(':')?;
    let (crate_name_and_version, path) = input.split_once(':')?;
    let (crate_name, version) = crate_name_and_version.rsplit_once('-')?;
    Some((
        registry.to_owned(),
        crate_name.to_owned(),
        version.to_owned(),
        path.to_owned(),
    ))
}

fn parse_special_path(input: &str) -> Option<MappedPath> {
    let mapped_path = if let Some((repo, path, rev)) = git_path(input) {
        MappedPath::Git { repo, path, rev }
    } else if let Some((repo, path, rev)) = hg_path(input) {
        MappedPath::Hg { repo, path, rev }
    } else if let Some((bucket, digest, path)) = s3_path(input) {
        MappedPath::S3 {
            bucket,
            digest,
            path,
        }
    } else if let Some((registry, crate_name, version, path)) = cargo_path(input) {
        MappedPath::Cargo {
            registry,
            crate_name,
            version,
            path,
        }
    } else {
        return None;
    };
    Some(mapped_path)
}

fn github_url(input: &str) -> Option<(String, String, String)> {
    // Example: "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h"
    let input = input.strip_prefix("https://raw.githubusercontent.com/")?;
    let (org, input) = input.split_once('/')?;
    let (repo_name, input) = input.split_once('/')?;
    let (rev, path) = input.split_once('/')?;
    Some((
        format!("github.com/{org}/{repo_name}"),
        path.to_owned(),
        rev.to_owned(),
    ))
}

fn hg_url(input: &str) -> Option<(String, String, String)> {
    // Example: "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
    let input = input.strip_prefix("https://hg.")?;
    let (host_rest, input) = input.split_once('/')?;
    let (repo, input) = input.split_once("/raw-file/")?;
    let (rev, path) = input.split_once('/')?;
    Some((
        format!("hg.{host_rest}/{repo}"),
        path.to_owned(),
        rev.to_owned(),
    ))
}

fn s3_url(input: &str) -> Option<(String, String, String)> {
    // Example: "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
    let input = input.strip_prefix("https://")?;
    let (bucket, input) = input.split_once(".s3.amazonaws.com/")?;
    let (digest, path) = input.split_once('/')?;
    Some((bucket.to_owned(), digest.to_owned(), path.to_owned()))
}

fn parse_url(input: &str) -> Option<MappedPath> {
    let mapped_path = if let Some((repo, path, rev)) = github_url(input) {
        MappedPath::Git { repo, path, rev }
    } else if let Some((repo, path, rev)) = hg_url(input) {
        MappedPath::Hg { repo, path, rev }
    } else if let Some((bucket, digest, path)) = s3_url(input) {
        MappedPath::S3 {
            bucket,
            digest,
            path,
        }
    } else if let Some(mapped_path) = parse_gitiles_url(input) {
        mapped_path
    } else {
        return None;
    };
    Some(mapped_path)
}

fn parse_raw_path(raw_path: &str) -> Option<MappedPath> {
    if let Ok(p) = map_rustc_path(raw_path) {
        return Some(p);
    }
    if let Ok(p) = map_cargo_dep_path(raw_path) {
        return Some(p);
    }
    None
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

fn parse_gitiles_url(input: &str) -> Option<MappedPath> {
    // https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT
    // -> "git:pdfium.googlesource.com/pdfium:core/fdrm/fx_crypt.cpp:dab1161c861cc239e48a17e1a5d729aa12785a53"
    // https://chromium.googlesource.com/chromium/src.git/+/c15858db55ed54c230743eaa9678117f21d5517e/third_party/blink/renderer/core/svg/svg_point.cc?format=TEXT
    // -> "git:chromium.googlesource.com/chromium/src:third_party/blink/renderer/core/svg/svg_point.cc:c15858db55ed54c230743eaa9678117f21d5517e"
    let input = input
        .strip_prefix("https://")?
        .strip_suffix("?format=TEXT")?;
    let (repo, input) = input.split_once(".git/+/")?;
    let (rev, path) = input.split_once('/')?;
    Some(MappedPath::Git {
        repo: repo.to_owned(),
        path: path.to_owned(),
        rev: rev.to_owned(),
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_hg_paths() {
        assert_eq!(
            MappedPath::from_special_path_str(
                "hg:hg.mozilla.org/mozilla-central:widget/cocoa/nsAppShell.mm:997f00815e6bc28806b75448c8829f0259d2cb28"
            ),
            Some(MappedPath::Hg {
                repo: "hg.mozilla.org/mozilla-central".to_string(),
                path: "widget/cocoa/nsAppShell.mm".to_string(),
                rev: "997f00815e6bc28806b75448c8829f0259d2cb28".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
            ),
            Some(MappedPath::Hg {
                repo: "hg.mozilla.org/mozilla-central".to_string(),
                path: "mozglue/baseprofiler/core/ProfilerBacktrace.cpp".to_string(),
                rev: "1706d4d54ec68fae1280305b70a02cb24c16ff68".to_string(),
            })
        );
    }

    #[test]
    fn parse_git_paths() {
        assert_eq!(
            MappedPath::from_special_path_str(
                "git:github.com/rust-lang/rust:library/std/src/sys/unix/thread.rs:53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b"
            ),
            Some(MappedPath::Git {
                repo: "github.com/rust-lang/rust".to_string(),
                path: "library/std/src/sys/unix/thread.rs".to_string(),
                rev: "53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "git:chromium.googlesource.com/chromium/src:content/gpu/gpu_main.cc:4dac2548d4812df2aa4a90ac1fc8912363f4d59c"
            ),
            Some(MappedPath::Git {
                repo: "chromium.googlesource.com/chromium/src".to_string(),
                path: "content/gpu/gpu_main.cc".to_string(),
                rev: "4dac2548d4812df2aa4a90ac1fc8912363f4d59c".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "git:pdfium.googlesource.com/pdfium:core/fdrm/fx_crypt.cpp:dab1161c861cc239e48a17e1a5d729aa12785a53"
            ),
            Some(MappedPath::Git {
                repo: "pdfium.googlesource.com/pdfium".to_string(),
                path: "core/fdrm/fx_crypt.cpp".to_string(),
                rev: "dab1161c861cc239e48a17e1a5d729aa12785a53".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h"
            ),
            Some(MappedPath::Git {
                repo: "github.com/baldurk/renderdoc".to_string(),
                path: "renderdoc/data/glsl/gl_texsample.h".to_string(),
                rev: "v1.15".to_string(),
            })
        );
    }

    #[test]
    fn parse_s3_paths() {
        assert_eq!(
            MappedPath::from_special_path_str(
                "s3:gecko-generated-sources:a5d3747707d6877b0e5cb0a364e3cb9fea8aa4feb6ead138952c2ba46d41045297286385f0e0470146f49403e46bd266e654dfca986de48c230f3a71c2aafed4/ipc/ipdl/PBackgroundChild.cpp:"
            ),
            Some(MappedPath::S3 {
                bucket: "gecko-generated-sources".to_string(),
                path: "ipc/ipdl/PBackgroundChild.cpp".to_string(),
                digest:
                "a5d3747707d6877b0e5cb0a364e3cb9fea8aa4feb6ead138952c2ba46d41045297286385f0e0470146f49403e46bd266e654dfca986de48c230f3a71c2aafed4".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "s3:gecko-generated-sources:4fd754dd7ca7565035aaa3357b8cd99959a2dddceba0fc2f7018ef99fd78ea63d03f9bf928afdc29873089ee15431956791130b97f66ab8fcb88ec75f4ba6b04/aarch64-apple-darwin/release/build/swgl-580c7d646d09cf59/out/ps_text_run_ALPHA_PASS_TEXTURE_2D.h:"
            ),
            Some(MappedPath::S3 {
                bucket: "gecko-generated-sources".to_string(),
                path: "aarch64-apple-darwin/release/build/swgl-580c7d646d09cf59/out/ps_text_run_ALPHA_PASS_TEXTURE_2D.h".to_string(),
                digest: "4fd754dd7ca7565035aaa3357b8cd99959a2dddceba0fc2f7018ef99fd78ea63d03f9bf928afdc29873089ee15431956791130b97f66ab8fcb88ec75f4ba6b04".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
            ),
            Some(MappedPath::S3 {
                bucket: "gecko-generated-sources".to_string(),
                path: "x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h".to_string(),
                digest: "7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3".to_string(),
            })
        );
    }

    #[test]
    fn parse_cargo_paths() {
        assert_eq!(
            MappedPath::from_special_path_str(
                "cargo:github.com-1ecc6299db9ec823:addr2line-0.16.0:src/function.rs"
            ),
            Some(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".to_string(),
                crate_name: "addr2line".to_string(),
                version: "0.16.0".to_string(),
                path: "src/function.rs".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "cargo:github.com-1ecc6299db9ec823:tokio-1.6.1:src/runtime/task/mod.rs"
            ),
            Some(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".to_string(),
                crate_name: "tokio".to_string(),
                version: "1.6.1".to_string(),
                path: "src/runtime/task/mod.rs".to_string(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "cargo:github.com-1ecc6299db9ec823:fxprof-processed-profile-0.3.0:src/lib.rs"
            ),
            Some(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".to_string(),
                crate_name: "fxprof-processed-profile".to_string(),
                version: "0.3.0".to_string(),
                path: "src/lib.rs".to_string(),
            })
        );
    }

    fn test_roundtrip(s: &str) {
        let mapped_path = MappedPath::from_special_path_str(s).unwrap();
        let roundtripped = mapped_path.to_special_path_str();
        assert_eq!(&roundtripped, s);
    }

    #[test]
    fn roundtrips() {
        test_roundtrip("hg:hg.mozilla.org/mozilla-central:widget/cocoa/nsAppShell.mm:997f00815e6bc28806b75448c8829f0259d2cb28");
        test_roundtrip("git:github.com/rust-lang/rust:library/std/src/sys/unix/thread.rs:53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b");
        test_roundtrip("git:chromium.googlesource.com/chromium/src:content/gpu/gpu_main.cc:4dac2548d4812df2aa4a90ac1fc8912363f4d59c");
        test_roundtrip("git:pdfium.googlesource.com/pdfium:core/fdrm/fx_crypt.cpp:dab1161c861cc239e48a17e1a5d729aa12785a53");
        test_roundtrip("s3:gecko-generated-sources:a5d3747707d6877b0e5cb0a364e3cb9fea8aa4feb6ead138952c2ba46d41045297286385f0e0470146f49403e46bd266e654dfca986de48c230f3a71c2aafed4/ipc/ipdl/PBackgroundChild.cpp:");
        test_roundtrip("s3:gecko-generated-sources:4fd754dd7ca7565035aaa3357b8cd99959a2dddceba0fc2f7018ef99fd78ea63d03f9bf928afdc29873089ee15431956791130b97f66ab8fcb88ec75f4ba6b04/aarch64-apple-darwin/release/build/swgl-580c7d646d09cf59/out/ps_text_run_ALPHA_PASS_TEXTURE_2D.h:");
        test_roundtrip("cargo:github.com-1ecc6299db9ec823:addr2line-0.16.0:src/function.rs");
        test_roundtrip("cargo:github.com-1ecc6299db9ec823:tokio-1.6.1:src/runtime/task/mod.rs");
        test_roundtrip(
            "cargo:github.com-1ecc6299db9ec823:fxprof-processed-profile-0.3.0:src/lib.rs",
        );
    }

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
                r"/rustc/e1884a8e3c3e813aada8254edfa120e85bf5ffca\/library\std\src\rt.rs"
            ),
            Ok(MappedPath::Git {
                repo: "github.com/rust-lang/rust".into(),
                path: "library/std/src/rt.rs".into(),
                rev: "e1884a8e3c3e813aada8254edfa120e85bf5ffca".into()
            })
        );
        assert_eq!(
            map_rustc_path(
                r"/rustc/a178d0322ce20e33eac124758e837cbd80a6f633\library\std\src\rt.rs"
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

    #[test]
    fn test_parse_gitiles_url() {
        assert_eq!(
            parse_gitiles_url("https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT"),
            Some(MappedPath::Git{
                repo: "pdfium.googlesource.com/pdfium".into(),
                rev: "dab1161c861cc239e48a17e1a5d729aa12785a53".into(),
                path: "core/fdrm/fx_crypt.cpp".into(),
            })
        );

        assert_eq!(
            parse_gitiles_url("https://chromium.googlesource.com/chromium/src.git/+/c15858db55ed54c230743eaa9678117f21d5517e/third_party/blink/renderer/core/svg/svg_point.cc?format=TEXT"),
            Some(MappedPath::Git{
                repo: "chromium.googlesource.com/chromium/src".into(),
                rev: "c15858db55ed54c230743eaa9678117f21d5517e".into(),
                path: "third_party/blink/renderer/core/svg/svg_point.cc".into(),
            })
        );

        assert_eq!(
            parse_gitiles_url("https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXTotherstuff"),
            None
        );
    }
}
