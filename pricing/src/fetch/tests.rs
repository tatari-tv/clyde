#![allow(clippy::unwrap_used)]

use std::sync::Mutex;
use std::time::Duration;

use super::*;

// Serialize the one env-var-touching test (XDG_CONFIG_HOME) so it cannot race
// its own set/restore. Other fetch tests only ever read the env indirectly and
// never plant a "test-app" override, so their embedded-fallback assertions hold
// regardless of this test's transient window.
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
