use nom::branch::alt;
use nom::bytes::complete::{tag, take_until1};
use nom::combinator::{eof, map};
use nom::error::ErrorKind;
use nom::sequence::terminated;
use nom::{Err, IResult};

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
        match parse_special_path(special_path) {
            Ok((_, mapped_path)) => Some(mapped_path),
            Err(_) => None,
        }
    }

    /// Detect some URLs of plain text files and convert them to a `MappedPath`.
    pub fn from_url(url: &str) -> Option<Self> {
        match parse_url(url) {
            Ok((_, mapped_path)) => Some(mapped_path),
            Err(_) => None,
        }
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
    pub fn display_path(&self) -> String {
        match self {
            MappedPath::Git { path, .. } => path.clone(),
            MappedPath::Hg { path, .. } => path.clone(),
            MappedPath::S3 { path, .. } => path.clone(),
            MappedPath::Cargo {
                crate_name,
                version,
                path,
                ..
            } => format!("{crate_name}-{version}/{path}"),
        }
    }
}

fn git_path(input: &str) -> IResult<&str, (String, String, String)> {
    let (input, _) = tag("git:")(input)?;
    let (input, repo) = terminated(take_until1(":"), tag(":"))(input)?;
    let (rev, path) = terminated(take_until1(":"), tag(":"))(input)?;
    Ok(("", (repo.to_owned(), path.to_owned(), rev.to_owned())))
}

fn hg_path(input: &str) -> IResult<&str, (String, String, String)> {
    let (input, _) = tag("hg:")(input)?;
    let (input, repo) = terminated(take_until1(":"), tag(":"))(input)?;
    let (rev, path) = terminated(take_until1(":"), tag(":"))(input)?;
    Ok(("", (repo.to_owned(), path.to_owned(), rev.to_owned())))
}

fn s3_path(input: &str) -> IResult<&str, (String, String, String)> {
    let (input, _) = tag("s3:")(input)?;
    let (input, bucket) = terminated(take_until1(":"), tag(":"))(input)?;
    let (input, digest) = terminated(take_until1("/"), tag("/"))(input)?;
    let (_, path) = terminated(take_until1(":"), terminated(tag(":"), eof))(input)?;
    Ok(("", (bucket.to_owned(), digest.to_owned(), path.to_owned())))
}

fn cargo_path(input: &str) -> IResult<&str, (String, String, String, String)> {
    let (input, _) = tag("cargo:")(input)?;
    let (input, registry) = terminated(take_until1(":"), tag(":"))(input)?;
    let (path, crate_name_and_version) = terminated(take_until1(":"), tag(":"))(input)?;
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
    Ok((
        "",
        (
            registry.to_owned(),
            crate_name.to_owned(),
            version.to_owned(),
            path.to_owned(),
        ),
    ))
}

fn parse_special_path(input: &str) -> IResult<&str, MappedPath> {
    alt((
        map(git_path, |(repo, path, rev)| MappedPath::Git {
            repo,
            path,
            rev,
        }),
        map(hg_path, |(repo, path, rev)| MappedPath::Hg {
            repo,
            path,
            rev,
        }),
        map(s3_path, |(bucket, digest, path)| MappedPath::S3 {
            bucket,
            digest,
            path,
        }),
        map(cargo_path, |(registry, crate_name, version, path)| {
            MappedPath::Cargo {
                registry,
                crate_name,
                version,
                path,
            }
        }),
    ))(input)
}

fn github_url(input: &str) -> IResult<&str, (String, String, String)> {
    // Example: "https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h"
    let (input, _) = tag("https://raw.githubusercontent.com/")(input)?;
    let (input, org) = terminated(take_until1("/"), tag("/"))(input)?;
    let (input, repo_name) = terminated(take_until1("/"), tag("/"))(input)?;
    let (input, rev) = terminated(take_until1("/"), tag("/"))(input)?;
    let path = input;
    Ok((
        "",
        (
            format!("github.com/{org}/{repo_name}"),
            path.to_owned(),
            rev.to_owned(),
        ),
    ))
}

fn hg_url(input: &str) -> IResult<&str, (String, String, String)> {
    // Example: "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
    let (input, _) = tag("https://hg.")(input)?;
    let (input, host_rest) = terminated(take_until1("/"), tag("/"))(input)?;
    let (input, repo) = terminated(take_until1("/raw-file/"), tag("/raw-file/"))(input)?;
    let (input, rev) = terminated(take_until1("/"), tag("/"))(input)?;
    let path = input;
    Ok((
        "",
        (
            format!("hg.{host_rest}/{repo}"),
            path.to_owned(),
            rev.to_owned(),
        ),
    ))
}

fn s3_url(input: &str) -> IResult<&str, (String, String, String)> {
    // Example: "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
    let (input, _) = tag("https://")(input)?;
    let (input, bucket) =
        terminated(take_until1(".s3.amazonaws.com/"), tag(".s3.amazonaws.com/"))(input)?;
    let (input, digest) = terminated(take_until1("/"), tag("/"))(input)?;
    let path = input;
    Ok(("", (bucket.to_owned(), digest.to_owned(), path.to_owned())))
}

fn parse_url(input: &str) -> IResult<&str, MappedPath> {
    alt((
        map(github_url, |(repo, path, rev)| MappedPath::Git {
            repo,
            path,
            rev,
        }),
        map(hg_url, |(repo, path, rev)| MappedPath::Hg {
            repo,
            path,
            rev,
        }),
        map(s3_url, |(bucket, digest, path)| MappedPath::S3 {
            bucket,
            digest,
            path,
        }),
    ))(input)
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
}
