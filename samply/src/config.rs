//! The per-user config file.
//!
//! The config file is a TOML file at `~/.config/samply/config.toml`
//! (`%APPDATA%\samply\config.toml` on Windows). It configures the profile
//! store, the symbol cache, and the symbol servers.
//!
//! This module must not depend on the CLI: a future daemon process will load
//! the config on its own.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use platform_dirs::AppDirs;
use samply_quota_manager::QuotaManager;
use serde_derive::Deserialize;

use crate::name::SAMPLY_NAME;
use crate::shared::prop_types::SymbolProps;

/// The name of the environment variable which overrides the config file path.
pub const CONFIG_PATH_ENV_VAR: &str = "SAMPLY_CONFIG";

/// The contents of the config file, with defaults filled in for missing keys.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub profiles: ProfilesConfig,
    pub symbols: SymbolsConfig,
}

/// The `[profiles]` section.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfilesConfig {
    /// The directory of the profile store. Defaults to `<data_dir>/profiles`.
    pub dir: Option<PathBuf>,
    /// Profiles which haven't been opened for this long are deleted.
    pub max_age: DurationLimit,
    /// Profiles opened or created within this span are never deleted.
    pub min_age: DurationLimit,
    /// Least-recently opened profiles are deleted when the store exceeds this size.
    pub max_total_size: SizeLimit,
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            dir: None,
            max_age: DurationLimit(Some(Duration::from_secs(30 * 24 * 60 * 60))),
            min_age: DurationLimit(Some(Duration::from_secs(24 * 60 * 60))),
            max_total_size: SizeLimit(Some(5 * 1000 * 1000 * 1000)),
        }
    }
}

impl ProfilesConfig {
    /// The profile store directory, with `~` expanded, falling back to the
    /// platform default. `None` if no home directory can be determined.
    pub fn resolved_dir(&self) -> Option<PathBuf> {
        match &self.dir {
            Some(dir) => Some(expand_tilde(dir)),
            None => Some(ConfigPaths::detect()?.default_profiles_dir),
        }
    }

    pub fn eviction(&self) -> EvictionConfig {
        EvictionConfig {
            max_age: self.max_age.0,
            min_age: self.min_age.0,
            max_total_size: self.max_total_size.0,
        }
    }
}

/// The `[symbols]` section.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SymbolsConfig {
    /// The base directory of the symbol cache. Defaults to `<cache_dir>/symbols`.
    pub cache_dir: Option<PathBuf>,
    pub max_age: DurationLimit,
    pub min_age: DurationLimit,
    pub max_total_size: SizeLimit,
    /// Same as `--symbol-dir`.
    pub symbol_dirs: Vec<PathBuf>,
    /// Same as `--windows-symbol-server`.
    pub windows_symbol_servers: Vec<String>,
    /// Same as `--windows-symbol-cache`.
    pub windows_symbol_cache: Option<PathBuf>,
    /// Same as `--breakpad-symbol-server`.
    pub breakpad_symbol_servers: Vec<String>,
    /// Same as `--breakpad-symbol-dir`.
    pub breakpad_symbol_dirs: Vec<String>,
    /// Same as `--breakpad-symbol-cache`.
    pub breakpad_symbol_cache: Option<PathBuf>,
    /// Same as `--simpleperf-binary-cache`.
    pub simpleperf_binary_cache: Option<PathBuf>,
    /// Look up symbols via debuginfod. Also enabled by the `SAMPLY_USE_DEBUGINFOD` env var.
    pub use_debuginfod: bool,
    /// Respect the `_NT_SYMBOL_PATH` environment variable.
    pub respect_nt_symbol_path: bool,
}

impl Default for SymbolsConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            max_age: DurationLimit(Some(Duration::from_secs(2 * 7 * 24 * 60 * 60))),
            min_age: DurationLimit(Some(Duration::from_secs(24 * 60 * 60))),
            max_total_size: SizeLimit(Some(10 * 1000 * 1000 * 1000)),
            symbol_dirs: Vec::new(),
            windows_symbol_servers: Vec::new(),
            windows_symbol_cache: None,
            breakpad_symbol_servers: Vec::new(),
            breakpad_symbol_dirs: Vec::new(),
            breakpad_symbol_cache: None,
            simpleperf_binary_cache: None,
            use_debuginfod: false,
            respect_nt_symbol_path: true,
        }
    }
}

impl SymbolsConfig {
    pub fn eviction(&self) -> EvictionConfig {
        EvictionConfig {
            max_age: self.max_age.0,
            min_age: self.min_age.0,
            max_total_size: self.max_total_size.0,
        }
    }

    /// Converts the config into symbol properties. CLI arguments are layered
    /// on top of the result afterwards.
    pub fn to_symbol_props(&self) -> SymbolProps {
        SymbolProps {
            symbol_dir: self.symbol_dirs.iter().map(|p| expand_tilde(p)).collect(),
            windows_symbol_server: self.windows_symbol_servers.clone(),
            windows_symbol_cache: self.windows_symbol_cache.as_deref().map(expand_tilde),
            breakpad_symbol_server: self.breakpad_symbol_servers.clone(),
            breakpad_symbol_dir: self.breakpad_symbol_dirs.clone(),
            breakpad_symbol_cache: self.breakpad_symbol_cache.as_deref().map(expand_tilde),
            simpleperf_binary_cache: self.simpleperf_binary_cache.as_deref().map(expand_tilde),
            symbol_cache_dir: self.cache_dir.as_deref().map(expand_tilde),
            eviction: self.eviction(),
            use_debuginfod: self.use_debuginfod,
            respect_nt_symbol_path: self.respect_nt_symbol_path,
        }
    }
}

/// Eviction settings for a directory managed by a [`QuotaManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvictionConfig {
    pub max_age: Option<Duration>,
    pub min_age: Option<Duration>,
    pub max_total_size: Option<u64>,
}

impl EvictionConfig {
    pub fn apply_to(&self, quota_manager: &QuotaManager) {
        quota_manager.set_max_age(self.max_age.map(|d| d.as_secs()));
        quota_manager.set_min_age(self.min_age.map(|d| d.as_secs()));
        quota_manager.set_max_total_size(self.max_total_size);
    }
}

/// An optional duration limit. Written as a humantime string such as `"30d"`
/// or `"2 weeks"`, or `"none"` to disable the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct DurationLimit(pub Option<Duration>);

impl TryFrom<String> for DurationLimit {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return Ok(Self(None));
        }
        humantime::parse_duration(value)
            .map(|d| Self(Some(d)))
            .map_err(|e| format!("invalid duration {value:?}: {e}"))
    }
}

/// An optional size limit. Written as a string such as `"5GB"` or `"500 MB"`,
/// or `"none"` to disable the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct SizeLimit(pub Option<u64>);

impl TryFrom<String> for SizeLimit {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return Ok(Self(None));
        }
        bytesize::ByteSize::from_str(value)
            .map(|b| Self(Some(b.as_u64())))
            .map_err(|e| format!("invalid size {value:?}: {e}"))
    }
}

/// The platform-specific default locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub config_file: PathBuf,
    pub default_profiles_dir: PathBuf,
    pub default_symbol_cache_dir: PathBuf,
}

impl ConfigPaths {
    /// Returns `None` if the home directory cannot be determined.
    pub fn detect() -> Option<Self> {
        // Config and data use XDG-style directories on all platforms, including
        // macOS, like most other command line tools. The symbol cache keeps
        // using the native cache directory (~/Library/Caches on macOS).
        let xdg_dirs = AppDirs::new(Some(SAMPLY_NAME), true)?;
        let native_dirs = AppDirs::new(Some(SAMPLY_NAME), false)?;
        Some(Self {
            config_file: xdg_dirs.config_dir.join("config.toml"),
            default_profiles_dir: xdg_dirs.data_dir.join("profiles"),
            default_symbol_cache_dir: native_dirs.cache_dir.join("symbols"),
        })
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    ExplicitPathMissing(PathBuf),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, source } => {
                write!(f, "Could not read config file {}: {source}", path.display())
            }
            ConfigError::Parse { path, source } => {
                write!(
                    f,
                    "Could not parse config file {}:\n{source}",
                    path.display()
                )
            }
            ConfigError::ExplicitPathMissing(path) => {
                write!(f, "Config file {} does not exist", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Loads the config file.
///
/// The path is `explicit_path` if given, otherwise the `SAMPLY_CONFIG`
/// environment variable if set, otherwise the platform default.
///
/// If the file at the default location does not exist, a commented template
/// is written there and the defaults are returned. Failure to write the
/// template is only logged.
pub fn load(explicit_path: Option<&Path>) -> Result<Config, ConfigError> {
    let (path, is_default_location) = match explicit_path {
        Some(path) => (path.to_path_buf(), false),
        None => match std::env::var_os(CONFIG_PATH_ENV_VAR) {
            Some(path) => (PathBuf::from(path), false),
            None => match ConfigPaths::detect() {
                Some(paths) => (paths.config_file, true),
                None => return Ok(Config::default()),
            },
        },
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => parse(&contents).map_err(|source| ConfigError::Parse { path, source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if is_default_location {
                write_template(&path);
                Ok(Config::default())
            } else {
                Err(ConfigError::ExplicitPathMissing(path))
            }
        }
        Err(source) => Err(ConfigError::Read { path, source }),
    }
}

pub fn parse(contents: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(contents)
}

fn write_template(path: &Path) {
    let Some(paths) = ConfigPaths::detect() else {
        return;
    };
    let contents = render_template(&paths);
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, contents));
    match result {
        Ok(()) => log::info!("Wrote default config file to {}", path.display()),
        Err(e) => log::warn!("Could not write config file {}: {e}", path.display()),
    }
}

const CONFIG_TEMPLATE: &str = r#"# samply configuration file.
#
# Every setting is optional. The values shown are the defaults.
# Durations are written like "30d", "2 weeks" or "12h".
# Sizes are written like "5GB" or "500MB". Use "none" to disable a limit.
# Paths may start with "~/".
# To change a setting, remove the # at the start of its line.

[profiles]
# Where `samply record` and `samply import` store profiles when no `-o` is given.
#dir = '{profiles_dir}'
# Delete profiles that haven't been opened for this long.
#max_age = "30d"
# Delete least-recently-opened profiles when the store exceeds this size.
#max_total_size = "5GB"
# Never delete profiles created or opened within this time span, even if the
# store is over its size limit.
#min_age = "1d"

[symbols]
# Where downloaded symbol files are cached.
#cache_dir = '{symbols_dir}'
#max_age = "2 weeks"
#max_total_size = "10GB"
#min_age = "1d"

# Extra directories containing symbol files (same as --symbol-dir).
#symbol_dirs = []
# Symbol servers serving PDB / DLL / EXE files (same as --windows-symbol-server).
{windows_symbol_servers_line}
#windows_symbol_cache = '{symbols_dir}/windows'
# Symbol servers serving Breakpad .sym files (same as --breakpad-symbol-server).
#breakpad_symbol_servers = ["https://symbols.mozilla.org/try/"]
#breakpad_symbol_dirs = []
#breakpad_symbol_cache = '{symbols_dir}/breakpad'
# Directory with the layout used by simpleperf's scripts (same as --simpleperf-binary-cache).
#simpleperf_binary_cache = ''
# Look up symbols via debuginfod (Linux). Also enabled by the SAMPLY_USE_DEBUGINFOD env var.
#use_debuginfod = false
# Respect the _NT_SYMBOL_PATH environment variable.
#respect_nt_symbol_path = true
"#;

const MICROSOFT_SYMBOL_SERVER_URL: &str = "https://msdl.microsoft.com/download/symbols";

fn render_template(paths: &ConfigPaths) -> String {
    let windows_symbol_servers_line = if cfg!(windows) {
        format!("windows_symbol_servers = [\"{MICROSOFT_SYMBOL_SERVER_URL}\"]")
    } else {
        format!("#windows_symbol_servers = [\"{MICROSOFT_SYMBOL_SERVER_URL}\"]")
    };
    CONFIG_TEMPLATE
        .replace(
            "{profiles_dir}",
            &paths.default_profiles_dir.display().to_string(),
        )
        .replace(
            "{symbols_dir}",
            &paths.default_symbol_cache_dir.display().to_string(),
        )
        .replace(
            "{windows_symbol_servers_line}",
            &windows_symbol_servers_line,
        )
}

/// Replaces a leading `~` with the home directory.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match std::env::home_dir() {
        Some(home) => home.join(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn test_paths() -> ConfigPaths {
        ConfigPaths {
            config_file: PathBuf::from("/home/me/.config/samply/config.toml"),
            default_profiles_dir: PathBuf::from("/home/me/.local/share/samply/profiles"),
            default_symbol_cache_dir: PathBuf::from("/home/me/.cache/samply/symbols"),
        }
    }

    #[test]
    fn template_parses_to_defaults() {
        let config = parse(&render_template(&test_paths())).unwrap();
        let mut expected = Config::default();
        if cfg!(windows) {
            expected.symbols.windows_symbol_servers = vec![MICROSOFT_SYMBOL_SERVER_URL.into()];
        }
        assert_eq!(config, expected);
    }

    #[test]
    fn empty_file_parses_to_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("[profiles]\n[symbols]\n").unwrap(), Config::default());
    }

    #[test]
    fn full_config_round_trips() {
        let config = parse(
            r#"
            [profiles]
            dir = "/tmp/profiles"
            max_age = "2 weeks"
            min_age = "none"
            max_total_size = "10GB"

            [symbols]
            cache_dir = "/tmp/symbols"
            max_age = "none"
            min_age = "30m"
            max_total_size = "none"
            symbol_dirs = ["/a", "/b"]
            windows_symbol_servers = ["https://example.com/w"]
            windows_symbol_cache = "/tmp/w"
            breakpad_symbol_servers = ["https://example.com/b"]
            breakpad_symbol_dirs = ["/bp"]
            breakpad_symbol_cache = "/tmp/b"
            simpleperf_binary_cache = "/tmp/s"
            use_debuginfod = true
            respect_nt_symbol_path = false
            "#,
        )
        .unwrap();
        assert_eq!(config.profiles.dir, Some(PathBuf::from("/tmp/profiles")));
        assert_eq!(
            config.profiles.max_age,
            DurationLimit(Some(Duration::from_secs(14 * 24 * 60 * 60)))
        );
        assert_eq!(config.profiles.min_age, DurationLimit(None));
        assert_eq!(
            config.profiles.max_total_size,
            SizeLimit(Some(10_000_000_000))
        );
        assert_eq!(
            config.symbols.cache_dir,
            Some(PathBuf::from("/tmp/symbols"))
        );
        assert_eq!(config.symbols.max_age, DurationLimit(None));
        assert_eq!(
            config.symbols.min_age,
            DurationLimit(Some(Duration::from_secs(30 * 60)))
        );
        assert_eq!(config.symbols.max_total_size, SizeLimit(None));
        assert_eq!(config.symbols.symbol_dirs.len(), 2);
        assert_eq!(
            config.symbols.windows_symbol_servers,
            ["https://example.com/w"]
        );
        assert_eq!(config.symbols.breakpad_symbol_dirs, ["/bp"]);
        assert!(config.symbols.use_debuginfod);
        assert!(!config.symbols.respect_nt_symbol_path);

        let props = config.symbols.to_symbol_props();
        assert_eq!(props.symbol_cache_dir, Some(PathBuf::from("/tmp/symbols")));
        assert_eq!(props.eviction.max_age, None);
        assert_eq!(props.eviction.min_age, Some(Duration::from_secs(30 * 60)));
        assert!(props.use_debuginfod);
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = parse("[profiles]\nbogus = 1\n").unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn invalid_duration_names_the_key() {
        let err = parse("[profiles]\nmax_age = \"bogus\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("max_age") || msg.contains("bogus"), "{msg}");
    }

    #[test]
    fn expand_tilde_only_replaces_leading_component() {
        assert_eq!(expand_tilde(Path::new("/x/~/y")), PathBuf::from("/x/~/y"));
        if let Some(home) = std::env::home_dir() {
            assert_eq!(expand_tilde(Path::new("~/y")), home.join("y"));
            assert_eq!(expand_tilde(Path::new("~")), home);
        }
    }
}
