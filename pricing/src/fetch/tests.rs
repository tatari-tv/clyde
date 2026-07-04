#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::sync::{Mutex, Once};
use std::time::Duration;

use super::*;

// Serialize the one env-var-touching test (XDG_CONFIG_HOME) so it cannot race
// its own set/restore. Other fetch tests only ever read the env indirectly and
// never plant a "test-app" override, so their embedded-fallback assertions hold
// regardless of this test's transient window.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// A capturing logger for the single-warn assertion (AC4/F5). The global logger
// is installed once, but capture is THREAD-LOCAL: each test runs on its own
// thread and all of our WARNs fire synchronously on that thread, so a capturing
// test sees exactly its own warns with zero cross-contamination from parallel
// tests (port reuse across tests defeats any URL-based filter on a shared buffer).
static LOG_INIT: Once = Once::new();

thread_local! {
    static TL_WARNS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if record.level() == log::Level::Warn {
            TL_WARNS.with(|w| {
                if let Some(buf) = w.borrow_mut().as_mut() {
                    buf.push(record.args().to_string());
                }
            });
        }
    }
    fn flush(&self) {}
}

fn install_capturing_logger() {
    LOG_INIT.call_once(|| {
        // Another logger being already set is fine; capture is opt-in per thread.
        let _ = log::set_boxed_logger(Box::new(CapturingLogger));
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// Run `f` with WARN capture enabled on the current thread and return its output
/// alongside the warns it emitted.
fn capture_warns<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    install_capturing_logger();
    TL_WARNS.with(|w| *w.borrow_mut() = Some(Vec::new()));
    let out = f();
    let warns = TL_WARNS.with(|w| w.borrow_mut().take().unwrap_or_default());
    (out, warns)
}

// V1_FEED carries a data_version far newer than the embedded baseline so that
// every fetch-success test exercises the "fetch newer wins" path: the Phase 9
// staleness guard now rejects any fetched feed older than embedded, and the
// embedded baseline's own data_version advances daily via the refresh cron.
// A fixed far-future date keeps these tests stable against that drift.
const V1_FEED: &str = r#"{
    "schema_version": 1,
    "data_version": "2099-01-01T00:00:00Z",
    "min_library_version": "0.0.0",
    "pricing": {
        "claude-opus-4-7": {
            "input_per_mtok": 5,
            "output_per_mtok": 25,
            "cache_5m_write_per_mtok": 6.25,
            "cache_1h_write_per_mtok": 10,
            "cache_read_per_mtok": 0.5
        }
    }
}"#;

fn test_config(server_url: &str, cache_dir: &Path, ttl: Duration, backoff: Duration) -> FetchConfig {
    FetchConfig {
        url: server_url.to_string(),
        cache_dir: cache_dir.to_path_buf(),
        ttl,
        failure_backoff: backoff,
    }
}

// A schema-1 feed body carrying `version` as its data_version (or none when
// `None`), otherwise identical to V1_FEED. Used to drive the staleness guard.
fn feed_with_version(version: Option<&str>) -> String {
    let dv = match version {
        Some(v) => format!("\"data_version\": \"{v}\",\n"),
        None => String::new(),
    };
    format!(
        r#"{{
    "schema_version": 1,
    {dv}    "min_library_version": "0.0.0",
    "pricing": {{
        "claude-opus-4-7": {{
            "input_per_mtok": 5,
            "output_per_mtok": 25,
            "cache_5m_write_per_mtok": 6.25,
            "cache_1h_write_per_mtok": 10,
            "cache_read_per_mtok": 0.5
        }}
    }}
}}"#
    )
}

#[test]
fn fetch_writes_cache_and_returns_pricing() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(V1_FEED)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Fetched { .. }));
    assert!(p.lookup("claude-opus-4-7").is_some());
    assert!(cfg.cache_path().exists(), "cache file should exist");
    assert!(!cfg.last_attempt_path().exists(), "no failure recorded on success");
}

#[test]
fn cache_hit_skips_network() {
    let mut server = mockito::Server::new();
    let mock = server.mock("GET", "/pricing.json").with_status(500).expect(0).create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn expired_cache_triggers_refetch() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(V1_FEED)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_secs(3600),
    );
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn fetch_failure_falls_back_to_cache() {
    let mut server = mockito::Server::new();
    let mock = server.mock("GET", "/pricing.json").with_status(503).create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_secs(3600),
    );
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(p.lookup("claude-opus-4-7").is_some(), "fell back to existing cache");
    assert!(cfg.last_attempt_path().exists(), "failure recorded");
}

#[test]
fn fetch_failure_with_no_cache_falls_back_to_embedded() {
    let mut server = mockito::Server::new();
    let mock = server.mock("GET", "/pricing.json").with_status(503).create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(cfg.last_attempt_path().exists(), "failure recorded");
}

#[test]
fn malformed_response_falls_back_and_records_failure() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body("not json")
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(cfg.last_attempt_path().exists());
    assert!(
        !cfg.cache_path().exists(),
        "malformed feed must not be written to cache"
    );
}

#[test]
fn failure_backoff_suppresses_repeat_fetches() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(V1_FEED)
        .expect(0)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    std::fs::create_dir_all(&cfg.cache_dir).unwrap();
    std::fs::write(cfg.last_attempt_path(), b"").unwrap();

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
}

#[test]
fn failure_backoff_with_cache_uses_cache() {
    let mut server = mockito::Server::new();
    let mock = server.mock("GET", "/pricing.json").with_status(200).expect(0).create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_secs(3600),
    );
    std::fs::create_dir_all(&cfg.cache_dir).unwrap();
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    std::fs::write(cfg.last_attempt_path(), b"").unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Fetched { .. }));
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn timeout_enforced_when_server_hangs() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/pricing.json");

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream;
            std::thread::sleep(Duration::from_secs(30));
        }
    });

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(&url, dir.path(), Duration::from_secs(3600), Duration::from_secs(3600));

    let start = std::time::Instant::now();
    let p = auto_with_config("test-app", &cfg).unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "auto returned after {:?}, expected < 10s",
        elapsed
    );
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(cfg.last_attempt_path().exists());
}

#[test]
fn successful_fetch_clears_last_attempt() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(V1_FEED)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    std::fs::create_dir_all(&cfg.cache_dir).unwrap();
    std::fs::write(cfg.last_attempt_path(), b"").unwrap();
    std::thread::sleep(Duration::from_millis(10));
    std::fs::remove_file(cfg.last_attempt_path()).unwrap();

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Fetched { .. }));
    assert!(!cfg.last_attempt_path().exists());
}

#[test]
fn schema_mismatch_falls_back() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(r#"{"schema_version": 99, "pricing": {}}"#)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(cfg.last_attempt_path().exists());
    assert!(
        !cfg.cache_path().exists(),
        "schema-incompatible feed must not be written to cache"
    );
}

#[test]
fn incompatible_library_version_does_not_poison_cache() {
    // min_library_version far above the crate version: from_bytes returns
    // Ok(embedded) rather than Err, so the fix must reject it explicitly
    // before writing the cache.
    let feed = r#"{
        "schema_version": 1,
        "data_version": "2026-04-28T00:00:00Z",
        "min_library_version": "999.0.0",
        "pricing": {
            "claude-opus-4-7": {
                "input_per_mtok": 5,
                "output_per_mtok": 25,
                "cache_5m_write_per_mtok": 6.25,
                "cache_1h_write_per_mtok": 10,
                "cache_read_per_mtok": 0.5
            }
        }
    }"#;
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(feed)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(
        cfg.last_attempt_path().exists(),
        "failure recorded for incompatible feed"
    );
    assert!(
        !cfg.cache_path().exists(),
        "library-incompatible feed must not be written to cache"
    );
}

#[test]
fn incompatible_feed_preserves_existing_valid_cache() {
    // A stale-but-valid cache must survive a fetch that returns an
    // incompatible feed: the bad bytes never overwrite the good cache.
    let bad_feed = r#"{
        "schema_version": 1,
        "min_library_version": "999.0.0",
        "pricing": {}
    }"#;
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(bad_feed)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_secs(3600),
    );
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(
        matches!(p.source(), crate::feed::Source::Fetched { .. }),
        "served the preserved cache"
    );
    assert!(p.lookup("claude-opus-4-7").is_some(), "good cache still usable");
    let on_disk = std::fs::read_to_string(cfg.cache_path()).unwrap();
    assert_eq!(on_disk, V1_FEED, "valid cache must be left untouched");
}

// ---- Phase 9: pricing staleness guard (D2) ----

#[test]
fn newer_feed_wins_and_is_cached() {
    // A fetched feed newer than the embedded baseline is authoritative and
    // persisted to the cache.
    let body = feed_with_version(Some("2099-06-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Fetched { .. }));
    assert!(cfg.cache_path().exists(), "newer feed must be cached");
    assert!(!cfg.last_attempt_path().exists(), "no failure recorded on success");
}

#[test]
fn equal_version_feed_wins_and_is_cached() {
    // A fetched feed whose data_version equals the embedded baseline is NOT
    // stale (strictly-older is the bar); it wins and is cached.
    let embedded = crate::pricing::embedded_data_version().expect("embedded baseline carries a data_version");
    let body = feed_with_version(Some(embedded));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Fetched { .. }));
    assert!(cfg.cache_path().exists(), "equal-version feed must be cached");
}

#[test]
fn stale_feed_not_cached_embedded_wins() {
    // A reachable, schema-valid feed older than the embedded baseline loses:
    // it is never written to cache and resolution falls through to embedded.
    let body = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(
        matches!(p.source(), crate::feed::Source::Embedded),
        "stale feed loses to embedded"
    );
    assert!(!cfg.cache_path().exists(), "stale feed must NOT be written to cache");
    assert!(
        cfg.last_attempt_path().exists(),
        "stale fetch records a failure/backoff"
    );
}

#[test]
fn stale_feed_preserves_existing_valid_cache() {
    // A still-valid cache must survive a stale fetch: the older bytes never
    // overwrite the good cache, mirroring the incompatible-feed guarantee.
    let stale = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&stale)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_secs(3600),
    );
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(
        matches!(p.source(), crate::feed::Source::Fetched { .. }),
        "served the preserved cache"
    );
    let on_disk = std::fs::read_to_string(cfg.cache_path()).unwrap();
    assert_eq!(on_disk, V1_FEED, "stale feed must not overwrite the valid cache");
}

#[test]
fn versionless_feed_treated_as_stale() {
    // A feed carrying no data_version at all cannot be compared, so it is
    // treated as stale (loses to embedded) rather than winning by default.
    let body = feed_with_version(None);
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(!cfg.cache_path().exists(), "version-less feed must NOT be cached");
    assert!(cfg.last_attempt_path().exists());
}

#[test]
fn malformed_version_feed_treated_as_stale() {
    // A non-canonical-UTC data_version (here a bare date, not a Z-suffixed
    // timestamp) is not lexicographically comparable and is treated as stale.
    let body = feed_with_version(Some("2099-13-40"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(!cfg.cache_path().exists(), "malformed-version feed must NOT be cached");
    assert!(cfg.last_attempt_path().exists());
}

#[test]
fn non_z_offset_version_treated_as_stale() {
    // A valid RFC-3339 timestamp with a non-Z UTC offset is still not the
    // canonical Z form, so lexicographic comparison is unsound and it is stale.
    let body = feed_with_version(Some("2099-06-01T00:00:00+00:00"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(
        !cfg.cache_path().exists(),
        "non-Z-offset version feed must NOT be cached"
    );
}

#[test]
fn fractional_seconds_version_treated_as_stale() {
    // A fractional-seconds timestamp parses as RFC-3339 and ends in Z, but is NOT the fixed-width
    // canonical form, so lexicographic comparison is unsound (e.g. "...00.500Z" sorts before
    // "...00Z" because '.' < 'Z'). Even though this version is chronologically far newer than the
    // 2026 embedded baseline, the non-canonical format alone must make it lose. Without the
    // round-trip check in is_canonical_utc this feed would be wrongly accepted and cached.
    let body = feed_with_version(Some("2099-06-01T00:00:00.500Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(
        !cfg.cache_path().exists(),
        "fractional-seconds version feed must NOT be cached"
    );
}

#[test]
fn stale_feed_falls_through_to_user_override() {
    // The staleness guard routes rejection through the existing fallback_chain,
    // which keeps the user override ahead of embedded. A stale fetch with no
    // cache but a present override must resolve to the override, not embedded.
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();

    let app = "stale-override-chain-app";
    let cfg_home = tempfile::TempDir::new().unwrap();
    // SAFETY: serialized behind ENV_LOCK; restored before the lock is dropped.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", cfg_home.path()) };

    let override_dir = cfg_home.path().join(app);
    std::fs::create_dir_all(&override_dir).unwrap();
    std::fs::write(
        override_dir.join("pricing.json"),
        feed_with_version(Some("2099-06-01T00:00:00Z")),
    )
    .unwrap();

    let stale = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&stale)
        .create();

    let cache_dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        cache_dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let result = auto_with_config(app, &cfg);
    mock.assert();

    // SAFETY: serialized behind ENV_LOCK.
    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);

    let p = result.unwrap();
    assert!(
        matches!(p.source(), crate::feed::Source::UserOverride(_)),
        "user override keeps its chain position ahead of embedded on a stale fetch"
    );
    assert!(
        !cfg.cache_path().exists(),
        "stale feed must NOT be cached even when an override exists"
    );
}

// ---- Phase 1: stale-feed persistence + accessor (dedicated sidecar) ----

#[test]
fn stale_fetch_writes_sidecar_and_surfaces_info() {
    // A stale rejection persists the dedicated sidecar and surfaces stale_feed().
    let body = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(
        cfg.stale_feed_path().exists(),
        "stale rejection writes the dedicated sidecar"
    );
    let info = p.stale_feed().expect("stale_feed surfaced");
    assert_eq!(info.fetched.as_deref(), Some("2000-01-01T00:00:00Z"));
    assert!(!info.embedded.is_empty(), "embedded baseline version recorded");
    // D7: a custom (mockito) URL is persisted origin-only, never the full path.
    assert!(
        !info.url.contains("/pricing.json"),
        "custom feed URL persisted origin-only (D7): {}",
        info.url
    );
}

#[test]
fn stale_then_fresh_cache_hit_still_reports_stale() {
    // AC1/F2: after a stale rejection, a later tick that hits the FRESH CACHE
    // (no fetch) must still surface the persisted stale marker.
    let body = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .expect(1)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    // Step 1: stale fetch writes the sidecar; resolution falls back to embedded.
    let p1 = auto_with_config("test-app", &cfg).unwrap();
    assert!(matches!(p1.source(), crate::feed::Source::Embedded));
    assert!(p1.stale_feed().is_some());
    assert!(cfg.stale_feed_path().exists());

    // Step 2: a fresh valid cache now exists; the next tick serves it WITHOUT a
    // fetch and must still surface the sidecar-persisted stale state.
    std::fs::write(cfg.cache_path(), V1_FEED).unwrap();
    let p2 = auto_with_config("test-app", &cfg).unwrap();
    mock.assert(); // still exactly one fetch total
    assert!(
        matches!(p2.source(), crate::feed::Source::Fetched { .. }),
        "served the fresh cache without fetching"
    );
    assert!(
        p2.stale_feed().is_some(),
        "fresh-cache tick still reports stale (hydrated from sidecar)"
    );
}

#[test]
fn transient_error_after_stale_preserves_marker() {
    // AC2/F1: a stale rejection followed by a transient fetch error must NOT
    // clear the marker (only a clean fetch clears it).
    let dir = tempfile::TempDir::new().unwrap();

    // Step 1: stale-200 from server A writes the sidecar.
    let stale = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server_a = mockito::Server::new();
    let mock_a = server_a
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&stale)
        .create();
    let cfg_a = test_config(
        &format!("{}/pricing.json", server_a.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    let p1 = auto_with_config("test-app", &cfg_a).unwrap();
    mock_a.assert();
    assert!(cfg_a.stale_feed_path().exists());
    assert!(p1.stale_feed().is_some());

    // Step 2: a transient 500 from server B. Tiny backoff (with a sleep) lets the
    // fetch actually run; the same cache_dir keeps the shared sidecar.
    let mut server_b = mockito::Server::new();
    let mock_b = server_b.mock("GET", "/pricing.json").with_status(500).create();
    let cfg_b = test_config(
        &format!("{}/pricing.json", server_b.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_millis(1),
    );
    std::thread::sleep(Duration::from_millis(50));
    let p2 = auto_with_config("test-app", &cfg_b).unwrap();
    mock_b.assert();
    assert!(
        cfg_b.stale_feed_path().exists(),
        "a transient fetch error must NOT clear the stale marker (F1)"
    );
    assert!(
        p2.stale_feed().is_some(),
        "stale state still surfaced after a transient error"
    );
}

#[test]
fn clean_fetch_after_stale_clears_marker() {
    // AC3: a stale rejection followed by a newer/equal-version 200 clears the
    // sidecar and stale_feed() returns None.
    let dir = tempfile::TempDir::new().unwrap();

    // Step 1: stale-200 from server A writes the sidecar.
    let stale = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server_a = mockito::Server::new();
    let mock_a = server_a
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&stale)
        .create();
    let cfg_a = test_config(
        &format!("{}/pricing.json", server_a.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    let _ = auto_with_config("test-app", &cfg_a).unwrap();
    assert!(cfg_a.stale_feed_path().exists());

    // Step 2: a newer-200 from server B is a clean fetch: it clears the sidecar.
    let newer = feed_with_version(Some("2099-06-01T00:00:00Z"));
    let mut server_b = mockito::Server::new();
    let mock_b = server_b
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&newer)
        .create();
    let cfg_b = test_config(
        &format!("{}/pricing.json", server_b.url()),
        dir.path(),
        Duration::from_millis(1),
        Duration::from_millis(1),
    );
    std::thread::sleep(Duration::from_millis(50));
    let p2 = auto_with_config("test-app", &cfg_b).unwrap();
    mock_b.assert();
    assert!(matches!(p2.source(), crate::feed::Source::Fetched { .. }));
    assert!(
        !cfg_b.stale_feed_path().exists(),
        "a clean fetch is the only event that clears the sidecar (AC3)"
    );
    assert!(p2.stale_feed().is_none(), "stale_feed cleared by the clean fetch");
    let _ = mock_a;
}

#[test]
fn single_stale_fetch_emits_exactly_one_warn() {
    // AC4/F5: one stale rejection logs exactly one WARN (the guard), never two.
    // The generic fetch-failure warn is suppressed for the StaleFeed variant.
    let body = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let (p, warns) = capture_warns(|| auto_with_config("test-app", &cfg).unwrap());
    mock.assert();
    assert!(p.stale_feed().is_some());
    assert_eq!(
        warns.len(),
        1,
        "a stale fetch must warn exactly once (the guard); got: {warns:?}"
    );
}

#[test]
fn legacy_last_attempt_suppresses_fetch_and_yields_no_stale() {
    // AC5/F4: an empty/legacy last_attempt still suppresses a fetch within the
    // backoff window and yields no stale info (last_attempt is backoff-only).
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(V1_FEED)
        .expect(0)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    std::fs::create_dir_all(&cfg.cache_dir).unwrap();
    std::fs::write(cfg.last_attempt_path(), b"").unwrap();

    let p = auto_with_config("test-app", &cfg).unwrap();
    mock.assert();
    assert!(matches!(p.source(), crate::feed::Source::Embedded));
    assert!(
        p.stale_feed().is_none(),
        "empty last_attempt is backoff-only; it must never hydrate stale info (F4)"
    );
    assert!(
        !cfg.stale_feed_path().exists(),
        "no stale sidecar is created by a backoff short-circuit"
    );
}

#[test]
fn refresh_persists_sidecar_on_stale_and_returns_stale_error() {
    // D5: the shared fetch boundary is used by refresh too. A stale fetch through
    // `refresh` persists the dedicated sidecar and returns the StaleFeed variant.
    let body = feed_with_version(Some("2000-01-01T00:00:00Z"));
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/pricing.json")
        .with_status(200)
        .with_body(&body)
        .create();

    let dir = tempfile::TempDir::new().unwrap();
    let cfg = test_config(
        &format!("{}/pricing.json", server.url()),
        dir.path(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    let result = refresh(&cfg);
    mock.assert();
    assert!(
        matches!(result, Err(PricingError::StaleFeed { .. })),
        "refresh surfaces the typed StaleFeed error"
    );
    assert!(
        cfg.stale_feed_path().exists(),
        "refresh persists the sidecar via the shared boundary"
    );
    let marker = read_stale_marker(&cfg).expect("sidecar readable");
    assert_eq!(marker.fetched.as_deref(), Some("2000-01-01T00:00:00Z"));
}
