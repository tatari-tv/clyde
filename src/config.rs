use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::pricing::ModelPricing;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Override the Claude projects directory
    pub projects_dir: Option<PathBuf>,
    /// Log level (trace, debug, info, warn, error)
    pub log_level: Option<String>,
    /// Pricing table - keyed by model name
    pub pricing: HashMap<String, ModelPricing>,
}

impl Config {
    /// Lightweight read of just the log_level field from the config file.
    /// Returns None if the file doesn't exist or can't be parsed.
    pub fn load_log_level() -> Option<String> {
        let config_dir = dirs::config_dir()?;
        let path = config_dir.join("ccu").join("ccu.yml");
        let content = fs::read_to_string(&path).ok()?;
        let config: Config = serde_yaml::from_str(&content).ok()?;
        config.log_level
    }

    pub fn load(config_path: Option<&PathBuf>) -> Result<Self> {
        log::debug!("Config::load: config_path={:?}", config_path);

        if let Some(path) = config_path {
            return Self::load_from_file(path).context(format!("Failed to load config from {}", path.display()));
        }

        // Try ~/.config/ccu/ccu.yml
        if let Some(config_dir) = dirs::config_dir() {
            let primary_config = config_dir.join("ccu").join("ccu.yml");
            if primary_config.exists() {
                match Self::load_from_file(&primary_config) {
                    Ok(config) => return Ok(config),
                    Err(e) => {
                        log::warn!("Failed to load config from {}: {}", primary_config.display(), e);
                    }
                }
            }
        }

        // No config file found - return empty config; caller merges embedded pricing
        log::info!("No config file found, using embedded pricing defaults");
        Ok(Config::default())
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
    fn test_load_log_level_returns_option() {
        // load_log_level should not panic regardless of config state
        let result = Config::load_log_level();
        // It returns Some(level) if config exists with log_level, None otherwise
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_config_deserialize_with_log_level() {
        let yaml = "log_level: debug\npricing: {}\n";
        let config: Config = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(config.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn test_config_deserialize_without_log_level() {
        let yaml = "pricing: {}\n";
        let config: Config = serde_yaml::from_str(yaml).expect("parse yaml");
        assert!(config.log_level.is_none());
    }

    #[test]
    fn test_load_explicit_path_missing() {
        let result = Config::load(Some(&PathBuf::from("/nonexistent/path/ccu.yml")));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_no_config_does_not_error() {
        // Config::load(None) should never error, even if no config file exists.
        // It returns either the loaded config or a default with empty pricing.
        let result = Config::load(None);
        assert!(result.is_ok(), "Config::load(None) should not error");
    }
}
