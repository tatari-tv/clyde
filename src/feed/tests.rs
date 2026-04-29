#![allow(clippy::unwrap_used)]

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

const LEGACY_FEED: &str = r#"{
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

fn write_feed(dir: &tempfile::TempDir, content: &str) -> std::path::PathBuf {
    let path = dir.path().join("pricing.json");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn embedded_loads_baseline_pricing() {
    let p = Pricing::embedded();
    assert!(matches!(p.source(), Source::Embedded));
    assert_eq!(p.schema_version(), CURRENT_SCHEMA_VERSION);
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn from_bytes_v1_shape_succeeds() {
    let p = Pricing::from_bytes(V1_FEED.as_bytes(), "v1.test".to_string(), Source::Embedded).unwrap();
    assert_eq!(p.schema_version(), 1);
    assert_eq!(p.data_version(), Some("2026-04-28T00:00:00Z"));
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn from_bytes_legacy_shape_succeeds() {
    let p = Pricing::from_bytes(LEGACY_FEED.as_bytes(), "legacy.test".to_string(), Source::Embedded).unwrap();
    assert_eq!(p.schema_version(), 1, "legacy feeds default to schema_version=1");
    assert_eq!(p.data_version(), None);
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn from_bytes_malformed_returns_error() {
    let result = Pricing::from_bytes(b"not json", "bad.test".to_string(), Source::Embedded);
    assert!(matches!(result, Err(PricingError::Malformed { .. })));
}

#[test]
fn from_bytes_unknown_schema_returns_error() {
    let json = r#"{"schema_version": 99, "pricing": {}}"#;
    let result = Pricing::from_bytes(json.as_bytes(), "future.test".to_string(), Source::Embedded);
    assert!(matches!(result, Err(PricingError::UnsupportedSchema { got: 99, .. })));
}

#[test]
fn from_bytes_min_library_too_high_falls_back_to_embedded() {
    let json = r#"{
        "schema_version": 1,
        "data_version": "2099-01-01T00:00:00Z",
        "min_library_version": "999.0.0",
        "pricing": {
            "claude-opus-4-7": {
                "input_per_mtok": 999.0,
                "output_per_mtok": 999.0,
                "cache_5m_write_per_mtok": 999.0,
                "cache_1h_write_per_mtok": 999.0,
                "cache_read_per_mtok": 999.0
            }
        }
    }"#;
    let p = Pricing::from_bytes(json.as_bytes(), "future.test".to_string(), Source::Embedded).unwrap();
    assert!(matches!(p.source(), Source::Embedded));
    let opus = p.lookup("claude-opus-4-7").unwrap();
    assert!(
        opus.input_per_mtok < 999.0,
        "fell back to embedded baseline, not feed pricing"
    );
}

#[test]
fn calculate_usd_via_pricing_struct() {
    let p = Pricing::embedded();
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    let cost = p.calculate_usd("claude-opus-4-7", &usage).unwrap();
    assert!(cost > 0.0);
}

#[test]
fn lookup_normalizes_bare_names() {
    let p = Pricing::embedded();
    assert!(p.lookup("opus").is_some());
    assert!(p.lookup("claude-opus-4-7-20251231").is_some());
}

#[test]
fn with_user_override_missing_falls_back_to_embedded() {
    let p = Pricing::with_user_override("definitely-not-a-real-app-name-12345").unwrap();
    assert!(matches!(p.source(), Source::Embedded));
}

#[test]
fn with_user_override_loads_from_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = write_feed(&dir, V1_FEED);

    let p = Pricing::load_from_path(&target, |path| Source::UserOverride(path.to_path_buf())).unwrap();
    match p.source() {
        Source::UserOverride(path) => assert_eq!(path, &target),
        other => panic!("expected UserOverride source, got {:?}", other),
    }
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn malformed_override_falls_through_to_embedded() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("pricing.json");
    std::fs::write(&target, "not json").unwrap();

    let result = Pricing::load_from_path(&target, |path| Source::UserOverride(path.to_path_buf()));
    assert!(matches!(result, Err(PricingError::Malformed { .. })));
}

#[test]
fn semver_compare_basic() {
    assert!(version_is_higher("1.0.0", "0.9.9"));
    assert!(version_is_higher("0.2.0", "0.1.99"));
    assert!(!version_is_higher("0.1.0", "0.1.0"));
    assert!(!version_is_higher("0.1.0", "1.0.0"));
}

#[test]
fn semver_handles_partial() {
    assert!(version_is_higher("2", "1.99.99"));
    assert!(!version_is_higher("1", "1.0.0"));
}

#[test]
fn semver_strips_prerelease() {
    assert!(!version_is_higher("1.0.0-alpha", "1.0.0"));
}

#[test]
fn from_bytes_records_source() {
    let url = "https://example.test/pricing.json".to_string();
    let fetched_at = chrono::Utc::now();
    let p = Pricing::from_bytes(
        V1_FEED.as_bytes(),
        url.clone(),
        Source::Fetched {
            url: url.clone(),
            fetched_at,
        },
    )
    .unwrap();
    match p.source() {
        Source::Fetched { url: u, .. } => assert_eq!(u, &url),
        other => panic!("expected Fetched source, got {:?}", other),
    }
}

#[test]
fn embedded_pricing_json_loads_via_feed() {
    let bytes = include_bytes!("../../data/pricing.json");
    let p = Pricing::from_bytes(bytes, "data/pricing.json".to_string(), Source::Embedded).unwrap();
    assert_eq!(p.schema_version(), 1);
    assert!(p.data_version().is_some(), "v1 feed should carry data_version");
    assert!(p.lookup("claude-opus-4-7").is_some());
}

#[test]
fn user_override_path_returns_app_specific_dir() {
    let path = user_override_path("test-app");
    let path = path.unwrap();
    assert!(path.ends_with("test-app/pricing.json"));
}
