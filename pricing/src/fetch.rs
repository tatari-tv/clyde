//! Feed resolution and on-disk caching for the pricing feed.
//!
//! # Source-selection state machine
//!
//! `auto_with_config` resolves a `Pricing` from several sources. There is
//! exactly ONE point that writes the on-disk cache (`write_cache_atomic` inside
//! `fetch_and_cache`), and every rejection path is arranged so a bad or stale
//! feed can never reach it.
//!
//! ```text
//!                    ┌─ cache fresh (within TTL) ─────────────► cache-hit:   load_from_cache  (no network)
//!                    │
//!  auto_with_config ─┤─ in failure backoff window ────────────► backoff:     fallback_chain   (no network)
//!                    │
//!                    └─ else fetch_and_cache ──┬─ HTTP/IO error ──────────────► fetch-fail:  Err → record_failure → fallback_chain
//!                                              │
//!                                              ├─ malformed / schema-too-new /
//!                                              │  library-too-old (from_bytes) ─► fetch-fail:  Err (NOT cached) → fallback_chain
//!                                              │
//!                                              ├─ data_version < embedded, or
//!                                              │  missing / malformed version ──► fetch-stale: Err (NOT cached) → fallback_chain, warn! both versions + URL
//!                                              │
//!                                              └─ data_version >= embedded ─────► fetch-newer: write_cache_atomic  ◄── the single cache-write point
//!
//!  fallback_chain: existing on-disk cache ─► user override (~/.config/<app>/pricing.json) ─► embedded baseline
//! ```
//!
//! The staleness guard (`fetch-stale`) lives INSIDE `fetch_and_cache`, before
//! `write_cache_atomic`, precisely so a stale feed is rejected before it can
//! overwrite a newer cache or land on disk. A check at a higher composition
//! point (e.g. in `auto_with_config` after the fetch returns) would run *after*
//! the bytes were already written, poisoning the cache. The user override keeps
//! its position in `fallback_chain`: an explicit local override is the
//! operator's documented escape hatch even when embedded is newer.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::error::PricingError;
use crate::feed::{DEFAULT_FEED_URL, Pricing, Source, StaleFeedInfo};

const DEFAULT_TTL_HOURS: u64 = 24;
const DEFAULT_FAILURE_BACKOFF_HOURS: u64 = 1;
const CONNECT_TIMEOUT_SECS: u64 = 2;
const READ_TIMEOUT_SECS: u64 = 3;
const TTL_ENV: &str = "CLAUDE_PRICING_TTL_HOURS";
const FAILURE_BACKOFF_ENV: &str = "CLAUDE_PRICING_FAILURE_BACKOFF_HOURS";
const FEED_URL_ENV: &str = "CLAUDE_PRICING_FEED_URL";
const CACHE_FILENAME: &str = "pricing.json";
const LAST_ATTEMPT_FILENAME: &str = "pricing.json.last-attempt";
// Dedicated stale-feed sidecar, SEPARATE from `last_attempt` (which is
// backoff-timing only). Written on a stale rejection; deleted only on a clean
// non-stale fetch. Its lifecycle ("the published feed is known stale until
// replaced") is independent of the failure-backoff lifecycle, so the two must
// not share a file (D2/F1).
const STALE_FEED_FILENAME: &str = "stale_feed.json";

#[derive(Debug, Clone)]
pub(crate) struct FetchConfig {
    pub url: String,
    pub cache_dir: PathBuf,
    pub ttl: Duration,
    pub failure_backoff: Duration,
}

impl FetchConfig {
    pub fn from_env() -> Self {
        // Cache lives under the unified clyde home (was `claude-pricing`). Disposable: not
        // migrated by bootstrap, it simply refetches at the new path on first run.
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("clyde")
            .join("pricing");
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

    pub fn stale_feed_path(&self) -> PathBuf {
        self.cache_dir.join(STALE_FEED_FILENAME)
    }
}

/// On-disk shape of the dedicated stale-feed sidecar. Carries an extra `at`
/// timestamp (for humans/debugging) beyond the public `StaleFeedInfo` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StaleMarker {
    fetched: Option<String>,
    embedded: String,
    url: String,
    at: String,
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
    debug!("claude-pricing: auto_with_config app_name={} url={}", app_name, cfg.url);
    let cache = cfg.cache_path();
    if cache_is_fresh(&cache, cfg.ttl) {
        match load_from_cache(&cache, &cfg.url) {
            // Fresh-cache tick: no fetch happened, but a prior stale rejection
            // must still surface, so hydrate from the sidecar (D2/F2).
            Ok(p) => return Ok(p.with_stale_feed(read_stale_marker(cfg))),
            Err(e) => warn!(
                "claude-pricing: cache at {} unusable ({}); refetching",
                cache.display(),
                e
            ),
        }
    }

    if in_failure_backoff(&cfg.last_attempt_path(), cfg.failure_backoff) {
        debug!("claude-pricing: in failure backoff window; skipping fetch");
        // Backoff short-circuit: no fetch, but hydrate any persisted stale state.
        return Ok(fallback_chain(app_name, &cache, &cfg.url)?.with_stale_feed(read_stale_marker(cfg)));
    }

    match fetch_with_stale_persist(cfg) {
        // Clean fetch: `fetch_and_cache` cleared the sidecar (the only clearer),
        // so `stale_feed` is correctly None on the resolved pricing.
        Ok(p) => Ok(p),
        // Stale or transient error: resolve via the fallback chain and hydrate
        // the stale marker. On a stale rejection the marker was just written; on
        // a transient error a pre-existing marker is preserved (never cleared).
        Err(_) => Ok(fallback_chain(app_name, &cache, &cfg.url)?.with_stale_feed(read_stale_marker(cfg))),
    }
}

/// Shared `fetch_and_cache`-caller boundary for both `auto_with_config` and
/// `refresh` (D5). Runs the fetch and reconciles the dedicated stale-feed
/// sidecar:
/// - a clean fetch already cleared the sidecar inside `fetch_and_cache` (the
///   only clearer of it, F1);
/// - a `StaleFeed` rejection persists the sidecar and SUPPRESSES the generic
///   fetch-failure `warn!` (the guard already logged exactly once, D4/F5);
/// - any other fetch error emits the generic `warn!`.
///
/// Every error records a failure for backoff timing. The `Result` is returned
/// unchanged; each caller resolves its own fallback and hydrates `stale_feed`.
fn fetch_with_stale_persist(cfg: &FetchConfig) -> Result<Pricing, PricingError> {
    debug!("claude-pricing: fetch_with_stale_persist url={}", cfg.url);
    match fetch_and_cache(cfg) {
        Ok(p) => Ok(p),
        Err(PricingError::StaleFeed { fetched, embedded, url }) => {
            write_stale_marker(
                cfg,
                &StaleFeedInfo {
                    fetched: fetched.clone(),
                    embedded: embedded.clone(),
                    url: url.clone(),
                },
            );
            record_failure(&cfg.last_attempt_path());
            Err(PricingError::StaleFeed { fetched, embedded, url })
        }
        Err(e) => {
            warn!(
                "claude-pricing: fetch from {} failed ({}); entering backoff",
                cfg.url, e
            );
            record_failure(&cfg.last_attempt_path());
            Err(e)
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
    debug!("claude-pricing: refresh url={}", cfg.url);
    fetch_with_stale_persist(cfg)
}

/// Read the dedicated stale-feed sidecar, if present and parseable. A missing
/// file is `None` (not an error); a malformed file is logged and ignored.
pub(crate) fn read_stale_marker(cfg: &FetchConfig) -> Option<StaleFeedInfo> {
    let path = cfg.stale_feed_path();
    debug!("claude-pricing: read_stale_marker path={}", path.display());
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<StaleMarker>(&bytes) {
        Ok(m) => Some(StaleFeedInfo {
            fetched: m.fetched,
            embedded: m.embedded,
            url: m.url,
        }),
        Err(e) => {
            warn!(
                "claude-pricing: stale marker at {} unreadable ({}); ignoring",
                path.display(),
                e
            );
            None
        }
    }
}

/// Persist the dedicated stale-feed sidecar (atomically). Best-effort: a write
/// failure is logged, never propagated - staleness is observe-only.
fn write_stale_marker(cfg: &FetchConfig, info: &StaleFeedInfo) {
    let path = cfg.stale_feed_path();
    debug!(
        "claude-pricing: write_stale_marker path={} fetched={:?} embedded={} url={}",
        path.display(),
        info.fetched,
        info.embedded,
        info.url
    );
    let marker = StaleMarker {
        fetched: info.fetched.clone(),
        embedded: info.embedded.clone(),
        url: info.url.clone(),
        at: Utc::now().to_rfc3339(),
    };
    match serde_json::to_vec_pretty(&marker) {
        Ok(bytes) => {
            if let Err(e) = write_cache_atomic(&path, &bytes) {
                warn!("claude-pricing: cannot write stale marker at {}: {}", path.display(), e);
            }
        }
        Err(e) => warn!("claude-pricing: cannot serialize stale marker: {}", e),
    }
}

/// Delete the dedicated stale-feed sidecar. Called ONLY on a clean non-stale
/// fetch - the single event that clears stale state (F1 invariant). A missing
/// file is not an error.
fn clear_stale_marker(cfg: &FetchConfig) {
    let path = cfg.stale_feed_path();
    debug!("claude-pricing: clear_stale_marker path={}", path.display());
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!("claude-pricing: cannot clear stale marker at {}: {}", path.display(), e),
    }
}

/// The feed URL to persist/surface. For the default feed the full URL is kept;
/// for a custom `CLAUDE_PRICING_FEED_URL` only the origin (scheme+authority) is
/// persisted so a private path/query is never written to disk (D7).
fn feed_url_for_display(url: &str) -> String {
    if url == DEFAULT_FEED_URL { url.to_string() } else { origin_only(url) }
}

/// Reduce a URL to scheme+authority, dropping path/query/fragment. Pure string
/// splitting (no byte slicing) so a multibyte URL can never panic.
fn origin_only(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{authority}")
        }
        None => url.split('/').next().unwrap_or(url).to_string(),
    }
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
    debug!("claude-pricing: fetch_and_cache url={}", cfg.url);
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

    // Validate before writing: a malformed feed returns Err here, and an
    // incompatible feed (schema too new, or min_library_version above this
    // crate) returns Ok(embedded()) - not Err - via the fallback in
    // Pricing::from_bytes. Caching either would poison the cache (and, worse,
    // overwrite a still-valid older cache). Only persist genuinely fetched,
    // compatible bytes.
    let fetched_at = Utc::now();
    let pricing = Pricing::from_bytes(
        &bytes,
        cfg.url.clone(),
        Source::Fetched {
            url: cfg.url.clone(),
            fetched_at,
        },
    )?;
    if !matches!(pricing.source(), Source::Fetched { .. }) {
        return Err(PricingError::Fetch {
            url: cfg.url.clone(),
            message: "fetched feed is incompatible with this library".to_string(),
        });
    }

    // Staleness guard (D2): a reachable, schema-valid feed whose data_version is
    // older than the embedded baseline (or missing/malformed) must lose to the
    // newer embedded data. Treat it exactly like an invalid feed - reject before
    // the cache write so it never overwrites a newer cache nor lands on disk;
    // resolution then falls through fallback_chain (cache -> override ->
    // embedded). Placement before write_cache_atomic is load-bearing.
    let fetched_version = pricing.data_version();
    let embedded_version = crate::pricing::embedded_data_version();
    if fetched_feed_is_stale(fetched_version, embedded_version) {
        // The guard logs the staleness exactly once here; the shared caller
        // boundary suppresses the generic fetch-failure warn for this variant so
        // a stale fetch never double-warns (D4/F5).
        warn!(
            "claude-pricing: fetched feed from {} is stale (data_version={:?}) versus embedded baseline (data_version={:?}); not caching, preferring the newer embedded/cache data",
            cfg.url, fetched_version, embedded_version
        );
        return Err(PricingError::StaleFeed {
            fetched: fetched_version.map(str::to_string),
            embedded: embedded_version.map(str::to_string).unwrap_or_default(),
            url: feed_url_for_display(&cfg.url),
        });
    }

    write_cache_atomic(&cfg.cache_path(), &bytes)?;
    let _ = std::fs::remove_file(cfg.last_attempt_path());
    // A clean, non-stale fetch is the ONLY event that clears stale state (F1).
    clear_stale_marker(cfg);

    Ok(pricing)
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

/// Decide whether a fetched feed is stale relative to the embedded baseline.
///
/// Returns `true` (feed loses; embedded/cache should win) when the fetched
/// `data_version` is strictly older than embedded, or is missing/malformed.
/// Returns `false` (guard permits the feed) when the fetched version is equal
/// or newer, OR when the embedded baseline itself carries no comparable version
/// (the guard disables itself and falls open to pre-guard behavior rather than
/// treating every fetched feed as stale).
///
/// Comparison is lexicographic and is sound only for canonical UTC ISO-8601
/// timestamps (`YYYY-MM-DDTHH:MM:SSZ`); a non-canonical value on either side is
/// not comparable (see `is_canonical_utc`), so a non-canonical *fetched* version
/// is treated as stale and a non-canonical *embedded* version disables the guard.
fn fetched_feed_is_stale(fetched: Option<&str>, embedded: Option<&str>) -> bool {
    let Some(embedded) = embedded.filter(|e| is_canonical_utc(e)) else {
        debug!("claude-pricing: embedded baseline has no comparable data_version; staleness guard disabled");
        return false;
    };
    match fetched {
        Some(f) if is_canonical_utc(f) => f < embedded,
        _ => true,
    }
}

/// A `data_version` is comparable only when it is a canonical whole-second UTC
/// ISO-8601 timestamp: `YYYY-MM-DDTHH:MM:SSZ`. Lexicographic ordering is valid
/// only across this exact fixed-width form; anything else (a non-`Z` offset like
/// `+00:00`, fractional seconds like `...SS.fffZ`, a lowercase `z`, or
/// unparseable text) would compare as garbage and is rejected.
///
/// The check round-trips: a string is canonical iff it is byte-identical to the
/// canonical rendering of its own parsed value. That single equality rejects
/// every non-fixed-width variant at once (in particular fractional seconds,
/// which `DateTime::parse_from_rfc3339` otherwise accepts).
fn is_canonical_utc(s: &str) -> bool {
    match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => dt.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string() == s,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
