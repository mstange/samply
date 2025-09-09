use std::borrow::Cow;

use nom::branch::alt;
use nom::bytes::complete::{tag, take_till1, take_until1, take_while1};
use nom::character::complete::one_of;
use nom::error::ErrorKind;
use nom::sequence::{delimited, preceded, terminated, tuple};
use nom::Err;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum UnparsedMappedPath<'a> {
    Url(Cow<'a, str>),
    BreakpadSpecialPath(Cow<'a, str>),
    RawPath(Cow<'a, str>),
}

impl<'a> UnparsedMappedPath<'a> {
    pub fn parse(&self) -> Option<MappedPath<'_>> {
        match self {
            UnparsedMappedPath::Url(url) => MappedPath::from_url(url),
            UnparsedMappedPath::BreakpadSpecialPath(bsp) => MappedPath::from_special_path_str(bsp),
            UnparsedMappedPath::RawPath(raw) => MappedPath::from_raw_path(raw),
        }
    }
    pub fn display_path(&self) -> Option<Cow<'_, str>> {
        self.parse().map(|mp| mp.display_path())
    }
    pub fn special_path_str(&self) -> Option<Cow<'_, str>> {
        let mp = match self {
            UnparsedMappedPath::Url(url) => MappedPath::from_url(url),
            UnparsedMappedPath::BreakpadSpecialPath(bsp) => return Some(Cow::Borrowed(bsp)),
            UnparsedMappedPath::RawPath(raw) => MappedPath::from_raw_path(raw),
        };
        mp.map(|mp| mp.to_special_path_str().into())
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
pub enum MappedPath<'a> {
    /// A path to a file in a git repository.
    Git {
        /// The web host + root path where the repository is hosted, e.g. `"github.com/rust-lang/rust"`.
        repo: Cow<'a, str>,
        /// The path to this file inside the repository, e.g. `"library/std/src/sys/unix/thread.rs"`.
        path: Cow<'a, str>,
        /// The revision, e.g. `"53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b"`.
        rev: Cow<'a, str>,
    },
    /// A path to a file in a mercurial repository (hg).
    Hg {
        /// The web host + root path where the repository is hosted, e.g. `"hg.mozilla.org/mozilla-central"`.
        repo: Cow<'a, str>,
        /// The path to this file inside the repository, e.g. `"widget/cocoa/nsAppShell.mm"`.
        path: Cow<'a, str>,
        /// The revision, e.g. `"997f00815e6bc28806b75448c8829f0259d2cb28"`.
        rev: Cow<'a, str>,
    },
    /// A path to a file hosted in an S3 bucket.
    S3 {
        /// The name of the S3 bucket, e.g. `"gecko-generated-sources"` (which is hosted at `https://gecko-generated-sources.s3.amazonaws.com/`).
        bucket: Cow<'a, str>,
        /// The "digest" of the file, i.e. a long hash string of hex characters.
        digest: Cow<'a, str>,
        /// The path to this file inside the bucket, e.g. `"ipc/ipdl/PBackgroundChild.cpp"`.
        path: Cow<'a, str>,
    },
    /// A path to a file in a Rust package which is hosted in a cargo registry (usually on crates.io).
    Cargo {
        /// The name of the cargo registry, usually `"github.com-1ecc6299db9ec823"`.
        registry: Cow<'a, str>,
        /// The name of the package, e.g. `"tokio"`.
        crate_name: Cow<'a, str>,
        /// The version of the package, e.g. `"1.6.1"`.
        version: Cow<'a, str>,
        /// The path to this file inside the package, e.g. `"src/runtime/task/mod.rs"`.
        path: Cow<'a, str>,
    },
}

impl<'a> MappedPath<'a> {
    /// Parse a "special path" string. These types of strings are found in Breakpad
    /// .sym files on the Mozilla symbol server.
    ///
    /// So this parsing code basically exists here because this crate supports obtaining
    /// symbols from Breakpad symbol files, so that consumers don't have parse this
    /// syntax when looking up symbols from a `SymbolMap` from such a .sym file.
    pub fn from_special_path_str(special_path: &'a str) -> Option<Self> {
        parse_special_path(special_path)
    }

    /// Detect some URLs of plain text files and convert them to a `MappedPath`.
    pub fn from_url(url: &'a str) -> Option<Self> {
        parse_url(url)
    }

    pub fn from_raw_path(raw_path: &'a str) -> Option<Self> {
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
    pub fn display_path(&self) -> Cow<'a, str> {
        match self {
            MappedPath::Git { path, .. } => path.clone(),
            MappedPath::Hg { path, .. } => path.clone(),
            MappedPath::S3 { path, .. } => path.clone(),
            MappedPath::Cargo {
                crate_name,
                version,
                path,
                ..
            } => format!("{crate_name}-{version}/{path}").into(),
        }
    }
}

fn parse_git_path(input: &str) -> Option<MappedPath<'_>> {
    let input = input.strip_prefix("git:")?;
    let (repo, input) = input.split_once(':')?;
    let (path, rev) = input.split_once(':')?;
    Some(MappedPath::Git {
        repo: repo.into(),
        path: path.into(),
        rev: rev.into(),
    })
}

fn parse_hg_path(input: &str) -> Option<MappedPath<'_>> {
    let input = input.strip_prefix("hg:")?;
    let (repo, input) = input.split_once(':')?;
    let (path, rev) = input.split_once(':')?;
    Some(MappedPath::Hg {
        repo: repo.into(),
        path: path.into(),
        rev: rev.into(),
    })
}

fn parse_s3_path(input: &str) -> Option<MappedPath<'_>> {
    let input = input.strip_prefix("s3:")?;
    let (bucket, input) = input.split_once(':')?;
    let (digest, input) = input.split_once('/')?;
    let path = input.strip_suffix(':')?;
    Some(MappedPath::S3 {
        bucket: bucket.into(),
        path: path.into(),
        digest: digest.into(),
    })
}

fn parse_cargo_path(input: &str) -> Option<MappedPath<'_>> {
    let input = input.strip_prefix("cargo:")?;
    let (registry, input) = input.split_once(':')?;
    let (crate_name_and_version, path) = input.split_once(':')?;
    let (crate_name, version) = crate_name_and_version.rsplit_once('-')?;
    Some(MappedPath::Cargo {
        registry: registry.into(),
        crate_name: crate_name.into(),
        version: version.into(),
        path: path.into(),
    })
}

fn parse_special_path(input: &str) -> Option<MappedPath<'_>> {
    None.or_else(|| parse_git_path(input))
        .or_else(|| parse_hg_path(input))
        .or_else(|| parse_s3_path(input))
        .or_else(|| parse_cargo_path(input))
}

fn parse_github_url(input: &str) -> Option<MappedPath<'_>> {
    // Example: "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h"
    let input = input.strip_prefix("https://raw.githubusercontent.com/")?;
    let (org, input) = input.split_once('/')?;
    let (repo_name, input) = input.split_once('/')?;
    let (rev, path) = input.split_once('/')?;
    Some(MappedPath::Git {
        repo: format!("github.com/{org}/{repo_name}").into(),
        path: path.into(),
        rev: rev.into(),
    })
}

fn parse_hg_url(input: &str) -> Option<MappedPath<'_>> {
    // Example: "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
    let input = input.strip_prefix("https://")?;
    let (repo, input) = input.split_once("/raw-file/")?;
    let (rev, path) = input.split_once('/')?;
    Some(MappedPath::Hg {
        repo: repo.into(),
        path: path.into(),
        rev: rev.into(),
    })
}

fn parse_s3_url(input: &str) -> Option<MappedPath<'_>> {
    // Example: "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
    let input = input.strip_prefix("https://")?;
    let (bucket, input) = input.split_once(".s3.amazonaws.com/")?;
    let (digest, path) = input.split_once('/')?;
    Some(MappedPath::S3 {
        bucket: bucket.into(),
        digest: digest.into(),
        path: path.into(),
    })
}

fn parse_url(input: &str) -> Option<MappedPath<'_>> {
    None.or_else(|| parse_github_url(input))
        .or_else(|| parse_hg_url(input))
        .or_else(|| parse_s3_url(input))
        .or_else(|| parse_gitiles_url(input))
}

fn parse_raw_path(raw_path: &str) -> Option<MappedPath<'_>> {
    if let Ok(p) = map_rustc_path(raw_path) {
        return Some(p);
    }
    if let Ok(p) = map_cargo_dep_path(raw_path) {
        return Some(p);
    }
    None
}

fn map_rustc_path(input: &str) -> Result<MappedPath<'_>, nom::Err<nom::error::Error<&str>>> {
    // /rustc/c79419af0721c614d050f09b95f076da09d37b0d/library/std/src/rt.rs
    // /rustc/e1884a8e3c3e813aada8254edfa120e85bf5ffca\/library\std\src\rt.rs
    // /rustc/a178d0322ce20e33eac124758e837cbd80a6f633\library\std\src\rt.rs
    let (input, rev) = delimited(
        tag("/rustc/"),
        take_till1(|c| c == '/' || c == '\\'),
        take_while1(|c| c == '/' || c == '\\'),
    )(input)?;
    let path = if input.contains('\\') {
        Cow::Owned(input.replace('\\', "/"))
    } else {
        Cow::Borrowed(input)
    };
    Ok(MappedPath::Git {
        repo: "github.com/rust-lang/rust".into(),
        path,
        rev: rev.into(),
    })
}

fn map_cargo_dep_path(input: &str) -> Result<MappedPath<'_>, nom::Err<nom::error::Error<&str>>> {
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
    let path = if input.contains('\\') {
        Cow::Owned(input.replace('\\', "/"))
    } else {
        Cow::Borrowed(input)
    };
    Ok(MappedPath::Cargo {
        registry: registry.into(),
        crate_name: crate_name.into(),
        version: version.into(),
        path,
    })
}

fn parse_gitiles_url(input: &str) -> Option<MappedPath<'_>> {
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
        repo: repo.into(),
        path: path.into(),
        rev: rev.into(),
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
                repo: "hg.mozilla.org/mozilla-central".into(),
                path: "widget/cocoa/nsAppShell.mm".into(),
                rev: "997f00815e6bc28806b75448c8829f0259d2cb28".into(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
            ),
            Some(MappedPath::Hg {
                repo: "hg.mozilla.org/mozilla-central".into(),
                path: "mozglue/baseprofiler/core/ProfilerBacktrace.cpp".into(),
                rev: "1706d4d54ec68fae1280305b70a02cb24c16ff68".into(),
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
                repo: "github.com/rust-lang/rust".into(),
                path: "library/std/src/sys/unix/thread.rs".into(),
                rev: "53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b".into(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "git:chromium.googlesource.com/chromium/src:content/gpu/gpu_main.cc:4dac2548d4812df2aa4a90ac1fc8912363f4d59c"
            ),
            Some(MappedPath::Git {
                repo: "chromium.googlesource.com/chromium/src".into(),
                path: "content/gpu/gpu_main.cc".into(),
                rev: "4dac2548d4812df2aa4a90ac1fc8912363f4d59c".into(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "git:pdfium.googlesource.com/pdfium:core/fdrm/fx_crypt.cpp:dab1161c861cc239e48a17e1a5d729aa12785a53"
            ),
            Some(MappedPath::Git {
                repo: "pdfium.googlesource.com/pdfium".into(),
                path: "core/fdrm/fx_crypt.cpp".into(),
                rev: "dab1161c861cc239e48a17e1a5d729aa12785a53".into(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h"
            ),
            Some(MappedPath::Git {
                repo: "github.com/baldurk/renderdoc".into(),
                path: "renderdoc/data/glsl/gl_texsample.h".into(),
                rev: "v1.15".into(),
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
                bucket: "gecko-generated-sources".into(),
                path: "ipc/ipdl/PBackgroundChild.cpp".into(),
                digest:
                "a5d3747707d6877b0e5cb0a364e3cb9fea8aa4feb6ead138952c2ba46d41045297286385f0e0470146f49403e46bd266e654dfca986de48c230f3a71c2aafed4".into(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "s3:gecko-generated-sources:4fd754dd7ca7565035aaa3357b8cd99959a2dddceba0fc2f7018ef99fd78ea63d03f9bf928afdc29873089ee15431956791130b97f66ab8fcb88ec75f4ba6b04/aarch64-apple-darwin/release/build/swgl-580c7d646d09cf59/out/ps_text_run_ALPHA_PASS_TEXTURE_2D.h:"
            ),
            Some(MappedPath::S3 {
                bucket: "gecko-generated-sources".into(),
                path: "aarch64-apple-darwin/release/build/swgl-580c7d646d09cf59/out/ps_text_run_ALPHA_PASS_TEXTURE_2D.h".into(),
                digest: "4fd754dd7ca7565035aaa3357b8cd99959a2dddceba0fc2f7018ef99fd78ea63d03f9bf928afdc29873089ee15431956791130b97f66ab8fcb88ec75f4ba6b04".into(),
            })
        );
        assert_eq!(
            MappedPath::from_url(
                "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
            ),
            Some(MappedPath::S3 {
                bucket: "gecko-generated-sources".into(),
                path: "x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h".into(),
                digest: "7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3".into(),
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
                registry: "github.com-1ecc6299db9ec823".into(),
                crate_name: "addr2line".into(),
                version: "0.16.0".into(),
                path: "src/function.rs".into(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "cargo:github.com-1ecc6299db9ec823:tokio-1.6.1:src/runtime/task/mod.rs"
            ),
            Some(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".into(),
                crate_name: "tokio".into(),
                version: "1.6.1".into(),
                path: "src/runtime/task/mod.rs".into(),
            })
        );
        assert_eq!(
            MappedPath::from_special_path_str(
                "cargo:github.com-1ecc6299db9ec823:fxprof-processed-profile-0.3.0:src/lib.rs"
            ),
            Some(MappedPath::Cargo {
                registry: "github.com-1ecc6299db9ec823".into(),
                crate_name: "fxprof-processed-profile".into(),
                version: "0.3.0".into(),
                path: "src/lib.rs".into(),
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
