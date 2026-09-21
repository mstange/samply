//! The per-user profile store: the directory where `samply record` and
//! `samply import` put profiles when no output path is given.
//!
//! Eviction is handled by a [`QuotaManager`] whose inventory database lives
//! next to the store directory, outside of it.
//!
//! This module must not depend on the CLI: a future daemon process will own
//! the profile store.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use samply_quota_manager::{QuotaManager, QuotaManagerNotifier};

use crate::config::ProfilesConfig;

/// Files in the store always use this extension. Other code derives sibling
/// file names from the profile path (Windows ETL files, `.syms.json`), so this
/// must stay in sync with the previous `profile.jslb.gz` default.
pub const PROFILE_FILE_EXTENSION: &str = "jslb.gz";

pub struct ProfileStore {
    dir: PathBuf,
    quota_manager: Option<QuotaManager>,
    notifier: Option<QuotaManagerNotifier>,
}

#[derive(Debug)]
pub enum ProfileStoreError {
    NoDataDir,
    CreateDir {
        dir: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ProfileStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileStoreError::NoDataDir => {
                write!(f, "Could not determine the profile store directory")
            }
            ProfileStoreError::CreateDir { dir, source } => write!(
                f,
                "Could not create profile store directory {}: {source}",
                dir.display()
            ),
        }
    }
}

impl std::error::Error for ProfileStoreError {}

impl ProfileStore {
    /// Opens the store, creating the directory if needed.
    ///
    /// Must be called from within a tokio runtime context, because the
    /// [`QuotaManager`] spawns its eviction task.
    pub fn open(config: &ProfilesConfig) -> Result<Self, ProfileStoreError> {
        let dir = config.resolved_dir().ok_or(ProfileStoreError::NoDataDir)?;
        std::fs::create_dir_all(&dir).map_err(|source| ProfileStoreError::CreateDir {
            dir: dir.clone(),
            source,
        })?;
        let dir = dir.canonicalize().unwrap_or(dir);

        // The database is a sibling of the store directory, named after it:
        // `<parent>/profiles.db` for the default `<parent>/profiles`.
        let db_name = format!(
            "{}.db",
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "profiles".into())
        );
        let db_path = match dir.parent() {
            Some(parent) => parent.join(db_name),
            None => dir.join(db_name),
        };

        let quota_manager = match QuotaManager::new(&dir, &db_path) {
            Ok(quota_manager) => {
                config.eviction().apply_to(&quota_manager);
                Some(quota_manager)
            }
            Err(e) => {
                log::error!(
                    "Could not create QuotaManager with profile store database {}: {e}",
                    db_path.display()
                );
                None
            }
        };
        let notifier = quota_manager.as_ref().map(|qm| qm.notifier());

        Ok(Self {
            dir,
            quota_manager,
            notifier,
        })
    }

    /// Returns a path for a new profile: `<dir>/<timestamp>_<name>.jslb.gz`,
    /// with a numeric suffix if that file already exists.
    pub fn new_profile_path(&self, profile_name: &str, now: SystemTime) -> PathBuf {
        let base = format!(
            "{}_{}",
            timestamp_for_filename(now),
            sanitize_profile_name(profile_name)
        );
        let mut candidate = self.dir.join(format!("{base}.{PROFILE_FILE_EXTENSION}"));
        let mut n = 2;
        while candidate.exists() {
            candidate = self
                .dir
                .join(format!("{base}_{n}.{PROFILE_FILE_EXTENSION}"));
            n += 1;
        }
        candidate
    }

    /// Registers a freshly written profile and triggers an eviction pass.
    pub fn on_profile_saved(&self, path: &Path) {
        self.register_existing_file(path);
        self.trigger_eviction();
    }

    /// Registers a file which already exists in the store directory, for
    /// example a kept ETL file. Does nothing for missing files.
    pub fn register_existing_file(&self, path: &Path) {
        let Some(notifier) = &self.notifier else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(path) else {
            return;
        };
        notifier.on_file_created(path, metadata.len(), SystemTime::now());
    }

    /// Marks a profile as recently used. Paths outside the store are ignored.
    pub fn on_profile_accessed(&self, path: &Path) {
        if let Some(notifier) = &self.notifier {
            notifier.on_file_accessed(path, SystemTime::now());
        }
    }

    pub fn trigger_eviction(&self) {
        if let Some(notifier) = &self.notifier {
            notifier.trigger_eviction_if_needed();
        }
    }

    /// Waits for any running eviction to finish.
    pub async fn finish(self) {
        if let Some(quota_manager) = self.quota_manager {
            quota_manager.finish().await;
        }
    }
}

/// Formats a time as `YYYY-MM-DD_HH-MM-SS` in the local time zone.
pub fn timestamp_for_filename(time: SystemTime) -> String {
    match jiff::Zoned::try_from(time) {
        Ok(zoned) => zoned.strftime("%Y-%m-%d_%H-%M-%S").to_string(),
        Err(_) => "unknown-time".to_string(),
    }
}

/// Turns a profile name into something safe to use in a file name.
///
/// Keeps ASCII letters, digits, `.`, `_` and `-`. Everything else becomes
/// `_`, runs of `_` are collapsed, and leading / trailing `_` and `.` are
/// removed. The result is at most 60 bytes and never empty.
pub fn sanitize_profile_name(name: &str) -> String {
    const MAX_LEN: usize = 60;
    let mut out = String::with_capacity(name.len());
    let mut last_was_underscore = false;
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            c
        } else {
            '_'
        };
        if c == '_' {
            if last_was_underscore {
                continue;
            }
            last_was_underscore = true;
        } else {
            last_was_underscore = false;
        }
        out.push(c);
    }
    let out = out.trim_matches(['_', '.']);
    let out = if out.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out[..end].trim_end_matches(['_', '.'])
    } else {
        out
    };
    if out.is_empty() {
        "profile".to_string()
    } else {
        out.to_string()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_profile_name("dump_syms"), "dump_syms");
        assert_eq!(sanitize_profile_name("perf.data"), "perf.data");
        assert_eq!(sanitize_profile_name("All processes"), "All_processes");
        assert_eq!(sanitize_profile_name("PID 1234"), "PID_1234");
        assert_eq!(sanitize_profile_name("./my app/бин"), "my_app");
        assert_eq!(sanitize_profile_name("..."), "profile");
        assert_eq!(sanitize_profile_name(""), "profile");
        assert_eq!(sanitize_profile_name("a__b   c"), "a_b_c");
        let long = "x".repeat(200);
        assert_eq!(sanitize_profile_name(&long).len(), 60);
    }

    #[test]
    fn timestamp_shape() {
        let s = timestamp_for_filename(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
        );
        assert_eq!(s.len(), 19, "{s}");
        let bytes = s.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'_');
        assert_eq!(bytes[13], b'-');
        assert_eq!(bytes[16], b'-');
        assert!(s.starts_with("2023-11-1"), "{s}");
    }

    #[test]
    fn collision_suffix() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = ProfileStore {
            dir: temp_dir.path().to_path_buf(),
            quota_manager: None,
            notifier: None,
        };
        let now = SystemTime::now();
        let first = store.new_profile_path("my app", now);
        assert!(first.to_string_lossy().ends_with("_my_app.jslb.gz"));
        std::fs::write(&first, b"x").unwrap();
        let second = store.new_profile_path("my app", now);
        assert!(second.to_string_lossy().ends_with("_my_app_2.jslb.gz"));
        std::fs::write(&second, b"x").unwrap();
        let third = store.new_profile_path("my app", now);
        assert!(third.to_string_lossy().ends_with("_my_app_3.jslb.gz"));
    }
}
