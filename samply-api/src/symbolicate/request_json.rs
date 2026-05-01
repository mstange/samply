use std::fmt;

use serde::de::{self, Deserializer};
use serde::Deserialize;

#[derive(serde_derive::Deserialize, Debug)]
#[serde(untagged)]
pub enum Request {
    WithJobsList { jobs: Vec<Job> },
    JustOneJob(Job),
}

impl Request {
    pub fn jobs(&self) -> JobIterator<'_> {
        match self {
            Request::WithJobsList { jobs } => JobIterator::WithJobsList(jobs.iter()),
            Request::JustOneJob(job) => JobIterator::JustOneJob(std::iter::once(job)),
        }
    }
}

#[derive(serde_derive::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub memory_map: Vec<Lib>,
    pub stacks: Vec<RequestStack>,
}

/// A debug filename whose characters have gone through some validation.
///
/// Allowed characters: alphanumeric and `_.+{}[]:@<> ~-`.
///
/// This check is based on eliot's `VALID_DEBUG_FILENAME` regex:
/// `^([A-Za-z0-9_.+{}@<> ~-]*)$`, extended with `[]:` for kallsyms /
/// kernel module names.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct ValidDebugName(String);

impl ValidDebugName {
    pub fn new(s: String) -> Result<Self, String> {
        if !is_valid_debug_filename(&s) {
            return Err(format!("invalid debug_filename {s:?}"));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ValidDebugName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for ValidDebugName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(de::Error::custom)
    }
}

impl serde::Serialize for ValidDebugName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

fn is_valid_debug_filename(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'_' | b'.' | b'+' | b'{' | b'}' | b'[' | b']' | b':' | b'@' | b'<' | b'>' | b' ' | b'~' | b'-')
    })
}

/// A string whose characters have been validated to be hex digits.
///
/// Used for breakpad IDs in symbolication requests.
/// The string is *not* required to / be a fully parseable [`debugid::DebugId`]
/// — strict parsing happens later, per-lib, so that a single malformed ID
/// fails just that lib's symbolication rather than the whole request.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct HexString(String);

impl HexString {
    pub fn new(s: String) -> Result<Self, String> {
        if !is_hex_string(&s) {
            return Err(format!("invalid debug_id {s:?}"));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HexString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for HexString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(de::Error::custom)
    }
}

impl serde::Serialize for HexString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

fn is_hex_string(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(serde_derive::Deserialize, Debug, PartialEq, Eq, Hash, Clone)]
pub struct Lib {
    pub debug_name: ValidDebugName,
    pub breakpad_id: HexString,
}

#[derive(serde_derive::Deserialize, Debug)]
pub struct RequestStack(pub Vec<RequestFrame>);

#[derive(serde_derive::Deserialize, Debug)]
pub struct RequestFrame {
    /// index into memory_map
    pub module_index: u32,
    /// lib-relative memory offset
    pub address: u32,
}

pub enum JobIterator<'a> {
    WithJobsList(std::slice::Iter<'a, Job>),
    JustOneJob(std::iter::Once<&'a Job>),
}

impl<'a> Iterator for JobIterator<'a> {
    type Item = &'a Job;

    fn next(&mut self) -> Option<&'a Job> {
        match self {
            JobIterator::WithJobsList(it) => it.next(),
            JobIterator::JustOneJob(it) => it.next(),
        }
    }
}

#[cfg(test)]
mod test {
    use serde_json::Result;

    use super::super::request_json::{Lib, Request};

    #[test]
    fn valid_debug_filename_chars() {
        // All allowed characters
        let lib: Lib = serde_json::from_str(r#"["xul.pdb", "44E4EC8C"]"#).unwrap();
        assert_eq!(lib.debug_name.as_str(), "xul.pdb");

        // Spaces, tildes, braces, brackets, etc.
        let lib: Lib =
            serde_json::from_str(r#"["lib foo+bar{v2}@<arch> ~test-1", "AABB"]"#).unwrap();
        assert_eq!(lib.debug_name.as_str(), "lib foo+bar{v2}@<arch> ~test-1");

        // Square brackets — used for kallsyms / kernel module debug names
        let lib: Lib = serde_json::from_str(r#"["[kernel.kallsyms]", ""]"#).unwrap();
        assert_eq!(lib.debug_name.as_str(), "[kernel.kallsyms]");

        // Empty string is allowed
        let lib: Lib = serde_json::from_str(r#"["", ""]"#).unwrap();
        assert_eq!(lib.debug_name.as_str(), "");
    }

    #[test]
    fn invalid_debug_filename_rejected() {
        // Slash is not allowed
        assert!(serde_json::from_str::<Lib>(r#"["xul/pdb", "44E4EC8C"]"#).is_err());
        // Backslash is not allowed
        assert!(serde_json::from_str::<Lib>(r#"["xul\\pdb", "44E4EC8C"]"#).is_err());
        // Exclamation mark is not allowed
        assert!(serde_json::from_str::<Lib>(r#"["xul!pdb", "44E4EC8C"]"#).is_err());
    }

    #[test]
    fn invalid_debug_id_rejected() {
        // Non-hex character
        assert!(
            serde_json::from_str::<Lib>(r#"["xul.pdb", "44E4EC8C2F41492B9369D6B9A059577G"]"#)
                .is_err()
        );
        // Hyphen not allowed in debug_id (only in debug_filename)
        assert!(serde_json::from_str::<Lib>(r#"["xul.pdb", "44E4EC8C-2F41"]"#).is_err());
    }

    #[test]
    fn parse_job() -> Result<()> {
        let data = r#"
        {
            "jobs": [
                {
                  "memoryMap": [
                    [
                      "xul.pdb",
                      "44E4EC8C2F41492B9369D6B9A059577C2"
                    ],
                    [
                      "wntdll.pdb",
                      "D74F79EB1F8D4A45ABCD2F476CCABACC2"
                    ]
                  ],
                  "stacks": [
                    [
                      [0, 11723767],
                      [1, 65802]
                    ]
                  ]
                }
            ]
        }"#;

        let r: Request = serde_json::from_str(data)?;
        assert_eq!(r.jobs().count(), 1);
        Ok(())
    }

    #[test]
    fn parse_without_jobs_wrapper() -> Result<()> {
        let data = r#"
        {
            "memoryMap": [
              [
                "xul.pdb",
                "44E4EC8C2F41492B9369D6B9A059577C2"
              ],
              [
                "wntdll.pdb",
                "D74F79EB1F8D4A45ABCD2F476CCABACC2"
              ]
            ],
            "stacks": [
              [
                [0, 11723767],
                [1, 65802]
              ]
            ]
          }
          "#;

        let r: Request = serde_json::from_str(data)?;
        assert_eq!(r.jobs().count(), 1);
        Ok(())
    }
}
