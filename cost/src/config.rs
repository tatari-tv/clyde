use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// We deliberately do NOT use the `dirs` config/data helpers: those honor
/// `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` only on Linux. On macOS they resolve via system
/// APIs and return `~/Library/...`, ignoring the env vars. These helpers resolve to the
/// same XDG layout on every platform.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Override the Claude projects directory
    pub projects_dir: Option<PathBuf>,
    /// Log level (trace, debug, info, warn, error)
    pub log_level: Option<String>,
}

impl Config {
    /// Load config and return the resolved path it was loaded from (None if using defaults).
    pub fn load(config_path: Option<&PathBuf>) -> Result<(Self, Option<PathBuf>)> {
        log::debug!("Config::load: config_path={:?}", config_path);

        if let Some(path) = config_path {
            let config =
                Self::load_from_file(path).context(format!("Failed to load config from {}", path.display()))?;
            return Ok((config, Some(path.clone())));
        }

        // XDG config dir, resolved identically on every platform (see xdg_config_dir).
        if let Some(config_dir) = xdg_config_dir() {
            let primary_config = config_dir.join("ccu").join("ccu.yml");
            if primary_config.exists() {
                match Self::load_from_file(&primary_config) {
                    Ok(config) => return Ok((config, Some(primary_config))),
                    Err(e) => {
                        log::warn!("Failed to load config from {}: {}", primary_config.display(), e);
                    }
                }
            }
        }

        // No config file found - return empty config; library owns pricing
        log::info!("No config file found, using library defaults");
        Ok((Config::default(), None))
    }

    fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        let config: Self = serde_yaml::from_str(&content).context("Failed to parse config file")?;
        log::info!("Loaded config from: {}", path.as_ref().display());
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_deserialize_with_log_level() {
        let yaml = "log_level: debug\n";
        let config: Config = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn test_config_deserialize_without_log_level() {
        let yaml = "{}\n";
        let config: Config = serde_yaml::from_str(yaml).expect("parse yaml");
        assert!(config.log_level.is_none());
    }

    #[test]
    fn test_config_silently_ignores_legacy_pricing_field() {
        // Pre-migration ccu auto-wrote a `pricing:` field. After migration the field is
        // gone from the struct; serde must silently ignore it so existing configs keep parsing.
        let yaml = "log_level: warn\npricing:\n  claude-opus-4-6:\n    input_per_mtok: 5.0\n    output_per_mtok: 25.0\n    cache_5m_write_per_mtok: 6.25\n    cache_1h_write_per_mtok: 10.0\n    cache_read_per_mtok: 0.5\n";
        let config: Config = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.log_level.as_deref(), Some("warn"));
    }

    #[test]
    fn test_load_explicit_path_missing() {
        let result = Config::load(Some(&PathBuf::from("/nonexistent/path/ccu.yml")));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_no_config_does_not_error() {
        // Config::load(None) should never error, even if no config file exists.
        let result = Config::load(None);
        assert!(result.is_ok(), "Config::load(None) should not error");
    }

    #[test]
    fn test_load_returns_path_for_explicit_config() {
        let path = PathBuf::from("/nonexistent/path/ccu.yml");
        let result = Config::load(Some(&path));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_returns_none_path_when_no_config() {
        let result = Config::load(None);
        assert!(result.is_ok());
    }
}
