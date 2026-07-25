//! Feed resolution and on-disk caching for the pricing feed.
//!
//! # Source-selection state machine
//!
//! `auto_with_config` resolves a `Pricing` from several sources. There is
//! exactly ONE point that writes the on-disk cache (`write_cache_atomic` inside
//! `fetch_and_cache`), and every rejection path is arranged so a bad or stale
//! feed can never reach it.
//!
//! The governing invariant is a property of every READ, not just of the write
//! point: **never serve a feed older than the embedded baseline.** Whatever the
//! source, a candidate loses to embedded when its `data_version` is older,
//! missing, or not comparable. One predicate (`loses_to_embedded`) decides that
//! for all three sites below.
//!
//! ```text
//!                    ┌─ cache fresh (within TTL) ─┬─ loses to embedded ─► fall THROUGH to the fetch
//!                    │      load_cache_candidate  │                       (warn! once, cache NOT served)
//!                    │                            └─ else ─────────────► cache-hit:   (no network)
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
//!  fallback_chain: on-disk cache (load_cache_candidate; SKIPPED when it loses to embedded)
//!                    ─► user override (~/.config/<app>/pricing.json) ─► embedded baseline
//! ```
//!
//! The fetched-feed guard lives INSIDE `fetch_and_cache`, before
//! `write_cache_atomic`, precisely so a stale feed is rejected before it can
//! overwrite a newer cache or land on disk. A check at a higher composition
//! point (e.g. in `auto_with_config` after the fetch returns) would run *after*
//! the bytes were already written, poisoning the cache.
//!
//! The two CACHE-read gates sit at the callers instead, in the shared
//! `load_cache_candidate`, because rejecting a cache is a resolution-order
//! decision and belongs where the state machine is. Pushing it down into
//! `load_from_cache` (a deserializer) would hide the decision from this diagram
//! and make "did not parse" and "is out of date" the same `Err` at every call
//! site. The shared helper is what keeps a future third read site from skipping
//! the gate.
//!
//! Neither cache gate touches the stale-feed sidecar: `fetch_and_cache` remains
//! its only writer and only clearer. The sidecar means "the upstream FEED we
//! fetched was behind embedded", which is a different fact from "the cache on
//! disk is behind embedded", and it has three consumers that would all be told a
//! lie -- the banner, `cost`'s `--offline` path, and the shipped statusline
//! glyph. A `warn!` carries the observability instead.
//!
//! The user override keeps its position in `fallback_chain`: an explicit local
//! override is the operator's documented escape hatch even when embedded is
//! newer.

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
    // Whether the never-older-than-embedded gate has already logged this resolution. A cache consulted
    // twice (here, then again inside `fallback_chain`) is still one fact, so it warns exactly once.
    let mut warned_cache_loss = false;
    if cache_is_fresh(&cache, cfg.ttl) {
        match load_cache_candidate(&cache, &cfg.url, true) {
            // Fresh-cache tick: no fetch happened, but a prior stale rejection
            // must still surface, so hydrate from the sidecar (D2/F2).
            CacheCandidate::Usable(p) => return Ok(p.with_stale_feed(read_stale_marker(cfg))),
            // The cache is fresh by TTL but older than embedded. Fall THROUGH to the fetch rather
            // than returning: the very next step is the fetch that would replace this bad cache, so
            // the case self-heals in one tick when the network is reachable. It does NOT self-heal
            // while the failure-backoff window is open -- the fetch is skipped below, `fallback_chain`
            // rejects this same cache again, and resolution lands on embedded. That is correct output
            // on every tick, but the bad cache is re-read and re-rejected until backoff expires:
            // terminating, not self-healing.
            CacheCandidate::LosesToEmbedded => warned_cache_loss = true,
            CacheCandidate::Unusable(e) => {
                if let Some(e) = e {
                    warn!(
                        "claude-pricing: cache at {} unusable ({}); refetching",
                        cache.display(),
                        e
                    );
                }
            }
        }
    }

    if in_failure_backoff(&cfg.last_attempt_path(), cfg.failure_backoff) {
        debug!("claude-pricing: in failure backoff window; skipping fetch");
        // Backoff short-circuit: no fetch, but hydrate any persisted stale state.
        return Ok(
            fallback_chain(app_name, &cache, &cfg.url, !warned_cache_loss)?.with_stale_feed(read_stale_marker(cfg))
        );
    }

    match fetch_with_stale_persist(cfg) {
        // Clean fetch: `fetch_and_cache` cleared the sidecar (the only clearer),
        // so `stale_feed` is correctly None on the resolved pricing.
        Ok(p) => Ok(p),
        // Stale or transient error: resolve via the fallback chain and hydrate
        // the stale marker. On a stale rejection the marker was just written; on
        // a transient error a pre-existing marker is preserved (never cleared).
        Err(_) => {
            Ok(fallback_chain(app_name, &cache, &cfg.url, !warned_cache_loss)?.with_stale_feed(read_stale_marker(cfg)))
        }
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

/// cache -> user override -> embedded, with the never-older-than-embedded gate on the cache arm.
///
/// A cache that loses to embedded is SKIPPED here rather than returned, so resolution continues to the
/// user override and then embedded. The user override deliberately keeps its position and still beats
/// embedded even when embedded is newer: an explicit local override is the operator's documented escape
/// hatch (see the module docs). Only the cache arm changed.
fn fallback_chain(app_name: &str, cache: &Path, url: &str, warn_on_loss: bool) -> Result<Pricing, PricingError> {
    match load_cache_candidate(cache, url, warn_on_loss) {
        CacheCandidate::Usable(p) => Ok(p),
        CacheCandidate::LosesToEmbedded | CacheCandidate::Unusable(_) => Pricing::with_user_override(app_name),
    }
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

/// Thin public wrapper over [`read_stale_marker`] for consumers that resolve pricing themselves
/// (e.g. `cost`'s `--offline` path, which calls [`Pricing::with_user_override`] and therefore never
/// touches `auto_with_config`'s hydration). Builds the same [`FetchConfig`] that `auto`/`refresh` use,
/// so the sidecar path can never drift between the writer and this reader (Risk row: "statusline
/// shell path drifts from the Rust sidecar path" applies equally to any Rust-side reader).
///
/// The design doc's API section names this `read_stale_marker(app_name)`; the sidecar path is a
/// property of [`FetchConfig`]'s cache dir, not of an app name (Phase 1 noted this same seam
/// correction for the private `read_stale_marker`), so this wrapper takes no `app_name` either.
pub fn stale_marker() -> Option<StaleFeedInfo> {
    debug!("claude-pricing: stale_marker (public wrapper)");
    let cfg = FetchConfig::from_env();
    read_stale_marker(&cfg)
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
    if loses_to_embedded(fetched_version, embedded_version) {
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
fn loses_to_embedded(candidate: Option<&str>, embedded: Option<&str>) -> bool {
    let Some(embedded) = embedded.filter(|e| is_canonical_utc(e)) else {
        debug!("claude-pricing: embedded baseline has no comparable data_version; staleness guard disabled");
        return false;
    };
    match candidate {
        Some(c) if is_canonical_utc(c) => c < embedded,
        _ => true,
    }
}

/// What an on-disk cache is worth to a resolution, after the never-older-than-embedded gate.
///
/// Modeled as three cases rather than a `Result` because "on disk but older than embedded" and "did
/// not parse" are genuinely different facts with different handling, and collapsing them into one
/// `Err` at the call sites is the typed-values-at-seams rule in miniature. It is also why the gate
/// lives here at the callers rather than inside `load_from_cache`, which is a deserializer: a
/// resolution-order decision folded into it would be invisible to the state machine the module doc
/// exists to describe.
enum CacheCandidate {
    /// Usable: parsed, and at least as new as the embedded baseline.
    Usable(Pricing),
    /// On disk and parseable, but older than embedded (or carrying no comparable version), so it
    /// must not be served. Nothing is wrong with the FILE; it is simply beaten by embedded.
    LosesToEmbedded,
    /// Absent, unreadable, or unparseable. Carries the error when there was one to report.
    Unusable(Option<PricingError>),
}

/// Load the on-disk cache and apply the invariant: **never serve a feed older than the embedded
/// baseline**, whatever its source.
///
/// This is the ONE place either read site consults the cache, so a future third read site has an
/// obvious thing to call and cannot quietly skip the gate (AC-P7).
///
/// `warn_on_loss` exists because a single resolution can consult the cache TWICE -- the fresh-cache
/// hit in [`auto_with_config`], then again inside [`fallback_chain`] after the fetch path declines --
/// and one rejected cache is ONE fact. The first consultation warns; the second stays quiet. Both
/// failure shapes are real and both are wrong: warning at both sites double-logs on the
/// backoff-active path, and warning only at `auto_with_config` leaves a fallback-chain-only rejection
/// (the cache was past its TTL, then the fetch failed) entirely unlogged. This mirrors the
/// warn-once discipline the sibling guard already keeps in `fetch_with_stale_persist`.
fn load_cache_candidate(cache: &Path, url: &str, warn_on_loss: bool) -> CacheCandidate {
    if !cache.exists() {
        return CacheCandidate::Unusable(None);
    }
    let candidate = match load_from_cache(cache, url) {
        Ok(p) => p,
        Err(e) => return CacheCandidate::Unusable(Some(e)),
    };
    let cached_version = candidate.data_version();
    let embedded_version = crate::pricing::embedded_data_version();
    if loses_to_embedded(cached_version, embedded_version) {
        if warn_on_loss {
            warn!(
                "claude-pricing: cached feed at {} is older than the embedded baseline (cached data_version={:?}, embedded data_version={:?}); not serving it, preferring the newer embedded data",
                cache.display(),
                cached_version,
                embedded_version
            );
        }
        // NOTE: deliberately does NOT write the stale-feed sidecar. That sidecar means "the upstream
        // FEED we fetched was behind embedded", which is a different fact with three consumers --
        // `format_stale_banner`, `cost`'s `--offline` path, and the shipped statusline glyph. Writing
        // it from here would light a persistent glyph in the user's prompt for a condition that is
        // not upstream staleness and that they cannot act on. `fetch_and_cache` remains the only
        // writer AND the only clearer (F1). The `warn!` above carries the observability instead.
        return CacheCandidate::LosesToEmbedded;
    }
    CacheCandidate::Usable(candidate)
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
