use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::Utc;
use log::{debug, warn};

use crate::error::PricingError;
use crate::feed::{DEFAULT_FEED_URL, Pricing, Source};

const DEFAULT_TTL_HOURS: u64 = 24;
const DEFAULT_FAILURE_BACKOFF_HOURS: u64 = 1;
const CONNECT_TIMEOUT_SECS: u64 = 2;
const READ_TIMEOUT_SECS: u64 = 3;
const TTL_ENV: &str = "CLAUDE_PRICING_TTL_HOURS";
const FAILURE_BACKOFF_ENV: &str = "CLAUDE_PRICING_FAILURE_BACKOFF_HOURS";
const FEED_URL_ENV: &str = "CLAUDE_PRICING_FEED_URL";
const CACHE_FILENAME: &str = "pricing.json";
const LAST_ATTEMPT_FILENAME: &str = "pricing.json.last-attempt";

#[derive(Debug, Clone)]
pub(crate) struct FetchConfig {
    pub url: String,
    pub cache_dir: PathBuf,
    pub ttl: Duration,
    pub failure_backoff: Duration,
}

impl FetchConfig {
    pub fn from_env() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("claude-pricing");
        Self {
            url: std::env::var(FEED_URL_ENV).unwrap_or_else(|_| DEFAULT_FEED_URL.to_string()),
            cache_dir,
            ttl: Duration::from_secs(env_hours(TTL_ENV, DEFAULT_TTL_HOURS) * 3600),
            failure_backoff: Duration::from_secs(env_hours(FAILURE_BACKOFF_ENV, DEFAULT_FAILURE_BACKOFF_HOURS) * 3600),
        }
    }

    pub fn cache_path(&self) -> PathBuf {
        self.cache_dir.join(CACHE_FILENAME)
    }

    pub fn last_attempt_path(&self) -> PathBuf {
        self.cache_dir.join(LAST_ATTEMPT_FILENAME)
    }
}

fn env_hours(name: &str, default_hours: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_hours)
}

pub(crate) fn auto(app_name: &str) -> Result<Pricing, PricingError> {
    let cfg = FetchConfig::from_env();
    auto_with_config(app_name, &cfg)
}

pub(crate) fn auto_with_config(app_name: &str, cfg: &FetchConfig) -> Result<Pricing, PricingError> {
    let cache = cfg.cache_path();
    if cache_is_fresh(&cache, cfg.ttl) {
        match load_from_cache(&cache, &cfg.url) {
            Ok(p) => return Ok(p),
            Err(e) => warn!(
                "claude-pricing: cache at {} unusable ({}); refetching",
                cache.display(),
                e
            ),
        }
    }

    if in_failure_backoff(&cfg.last_attempt_path(), cfg.failure_backoff) {
        debug!("claude-pricing: in failure backoff window; skipping fetch");
        return fallback_chain(app_name, &cache, &cfg.url);
    }

    match fetch_and_cache(cfg) {
        Ok(p) => Ok(p),
        Err(e) => {
            warn!(
                "claude-pricing: fetch from {} failed ({}); entering backoff",
                cfg.url, e
            );
            record_failure(&cfg.last_attempt_path());
            fallback_chain(app_name, &cache, &cfg.url)
        }
    }
}

fn fallback_chain(app_name: &str, cache: &Path, url: &str) -> Result<Pricing, PricingError> {
    if cache.exists()
        && let Ok(p) = load_from_cache(cache, url)
    {
        return Ok(p);
    }
    Pricing::with_user_override(app_name)
}

pub(crate) fn refresh(cfg: &FetchConfig) -> Result<Pricing, PricingError> {
    fetch_and_cache(cfg)
}

fn cache_is_fresh(path: &Path, ttl: Duration) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age < ttl)
        .unwrap_or(false)
}

fn in_failure_backoff(path: &Path, backoff: Duration) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age < backoff)
        .unwrap_or(false)
}

fn record_failure(path: &Path) {
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!("claude-pricing: cannot create cache dir {}: {}", parent.display(), e);
        return;
    }
    if let Err(e) = std::fs::write(path, b"") {
        warn!("claude-pricing: cannot record failure at {}: {}", path.display(), e);
    }
}

fn load_from_cache(path: &Path, url: &str) -> Result<Pricing, PricingError> {
    let bytes = std::fs::read(path).map_err(|source| PricingError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let fetched_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|st| {
            chrono::DateTime::<Utc>::from_timestamp(st.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as i64, 0)
        })
        .unwrap_or_else(Utc::now);
    Pricing::from_bytes(
        &bytes,
        path.display().to_string(),
        Source::Fetched {
            url: url.to_string(),
            fetched_at,
        },
    )
}

fn fetch_and_cache(cfg: &FetchConfig) -> Result<Pricing, PricingError> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(CONNECT_TIMEOUT_SECS)))
        .timeout_recv_response(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
        .timeout_recv_body(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
        .build()
        .new_agent();

    let response = agent.get(&cfg.url).call().map_err(|e| PricingError::Fetch {
        url: cfg.url.clone(),
        message: e.to_string(),
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(PricingError::Fetch {
            url: cfg.url.clone(),
            message: format!("HTTP {status}"),
        });
    }

    let bytes = response.into_body().read_to_vec().map_err(|e| PricingError::Fetch {
        url: cfg.url.clone(),
        message: e.to_string(),
    })?;

    write_cache_atomic(&cfg.cache_path(), &bytes)?;
    let _ = std::fs::remove_file(cfg.last_attempt_path());

    let fetched_at = Utc::now();
    Pricing::from_bytes(
        &bytes,
        cfg.url.clone(),
        Source::Fetched {
            url: cfg.url.clone(),
            fetched_at,
        },
    )
}

fn write_cache_atomic(target: &Path, bytes: &[u8]) -> Result<(), PricingError> {
    let parent = target.parent().ok_or_else(|| PricingError::Malformed {
        source_label: target.display().to_string(),
        message: "cache path has no parent directory".to_string(),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| PricingError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|source| PricingError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    use std::io::Write;
    tmp.write_all(bytes).map_err(|source| PricingError::Io {
        path: tmp.path().to_path_buf(),
        source,
    })?;
    tmp.persist(target).map_err(|e| PricingError::Io {
        path: target.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
