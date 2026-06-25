#![allow(clippy::unwrap_used)]

use std::time::Duration;

use super::*;

const V1_FEED: &str = r#"{
    "schema_version": 1,
    "data_version": "2026-04-28T00:00:00Z",
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
