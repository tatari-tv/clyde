use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::error::PricingError;
use crate::parse::TokenUsage;
use crate::pricing::{ModelPricing, calculate_cost, default_pricing, normalize_model_id};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_FEED_URL: &str = "https://tatari-tv.github.io/claude-pricing/pricing.json";
const LIBRARY_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum Source {
    Embedded,
    UserOverride(PathBuf),
    Fetched { url: String, fetched_at: DateTime<Utc> },
}

#[derive(Debug, Clone)]
pub struct Pricing {
    schema_version: u32,
    data_version: Option<String>,
    pricing: HashMap<String, ModelPricing>,
    source: Source,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PricingFeed {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub data_version: Option<String>,
    #[serde(default = "default_min_library_version")]
    pub min_library_version: String,
    pub pricing: HashMap<String, ModelPricing>,
}

fn default_schema_version() -> u32 {
    1
}

fn default_min_library_version() -> String {
    "0.0.0".to_string()
}

impl Pricing {
    pub fn embedded() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            data_version: None,
            pricing: default_pricing().clone(),
            source: Source::Embedded,
        }
    }

    pub fn with_user_override(app_name: &str) -> Result<Self, PricingError> {
        let path = user_override_path(app_name);
        match path.as_ref().filter(|p| p.exists()) {
            Some(p) => match Self::load_from_path(p, |path| Source::UserOverride(path.to_path_buf())) {
                Ok(loaded) => Ok(loaded),
                Err(e) => {
                    warn!(
                        "claude-pricing: user override at {} unusable ({}); falling back to embedded baseline",
                        p.display(),
                        e
                    );
                    Ok(Self::embedded())
                }
            },
            None => Ok(Self::embedded()),
        }
    }

    pub(crate) fn load_from_path(path: &Path, source_for: impl FnOnce(&Path) -> Source) -> Result<Self, PricingError> {
        let bytes = std::fs::read(path).map_err(|source| PricingError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes, path.display().to_string(), source_for(path))
    }

    pub(crate) fn from_bytes(bytes: &[u8], source_label: String, source: Source) -> Result<Self, PricingError> {
        let feed: PricingFeed = serde_json::from_slice(bytes).map_err(|e| PricingError::Malformed {
            source_label: source_label.clone(),
            message: e.to_string(),
        })?;

        if feed.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(PricingError::UnsupportedSchema {
                got: feed.schema_version,
                max: CURRENT_SCHEMA_VERSION,
            });
        }

        if version_is_higher(&feed.min_library_version, LIBRARY_VERSION) {
            warn!(
                "claude-pricing: published feed at {} requires library >= {}; current is {}; falling back to embedded baseline",
                source_label, feed.min_library_version, LIBRARY_VERSION
            );
            return Ok(Self::embedded());
        }

        debug!(
            "claude-pricing: loaded feed from {} (schema_version={}, data_version={:?}, models={})",
            source_label,
            feed.schema_version,
            feed.data_version,
            feed.pricing.len()
        );

        Ok(Self {
            schema_version: feed.schema_version,
            data_version: feed.data_version,
            pricing: feed.pricing,
            source,
        })
    }

    pub fn lookup(&self, model: &str) -> Option<&ModelPricing> {
        let key = normalize_model_id(model);
        self.pricing.get(key)
    }

    pub fn calculate_usd(&self, model: &str, usage: &TokenUsage) -> Result<f64, PricingError> {
        let pricing = self
            .lookup(model)
            .ok_or_else(|| PricingError::UnknownModel(model.to_string()))?;
        Ok(calculate_cost(pricing, usage))
    }

    pub fn data_version(&self) -> Option<&str> {
        self.data_version.as_deref()
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn models(&self) -> impl Iterator<Item = (&String, &ModelPricing)> {
        self.pricing.iter()
    }

    #[cfg(feature = "fetch")]
    pub fn auto(app_name: &str) -> Result<Self, PricingError> {
        crate::fetch::auto(app_name)
    }

    #[cfg(feature = "fetch")]
    pub fn refresh(&mut self) -> Result<(), PricingError> {
        let cfg = crate::fetch::FetchConfig::from_env();
        let refreshed = crate::fetch::refresh(&cfg)?;
        *self = refreshed;
        Ok(())
    }
}

pub fn user_override_path(app_name: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(app_name).join("pricing.json"))
}

fn version_is_higher(required: &str, current: &str) -> bool {
    let req = parse_semver(required);
    let cur = parse_semver(current);
    match (req, cur) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests;
