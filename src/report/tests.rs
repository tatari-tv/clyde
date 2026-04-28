#![allow(clippy::unwrap_used)]

use super::*;
use crate::session::{SessionSummary, TokenTotals};
use std::collections::BTreeSet;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn sample_summary(sid: &str, title: Option<&str>) -> SessionSummary {
    SessionSummary {
        session_id: sid.into(),
        repo: Some("tatari-tv/claude-report".into()),
        cwd: Some(PathBuf::from("/home/u/r")),
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        models: BTreeSet::from(["claude-opus-4-7".to_string()]),
        tokens: TokenTotals {
            input: 100,
            output: 200,
            cache_5m_write: 50,
            cache_1h_write: 0,
            cache_read: 1000,
            total: 1350,
        },
        jsonl_paths: vec![PathBuf::from("/path/to/parent.jsonl")],
        title: title.map(str::to_string),
    }
}

#[test]
fn write_yaml_round_trips() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.yml");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("do the thing"));
    let count = write_yaml(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();
    assert_eq!(count, 1);

    let body = fs::read_to_string(&path).unwrap();
    let report: Report = serde_yaml::from_str(&body).unwrap();
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.host, "desk");
    assert_eq!(report.session_count, 1);
    let entry = &report.sessions["9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042"];
    assert_eq!(entry.title.as_deref(), Some("do the thing"));
    assert_eq!(entry.repo.as_deref(), Some("tatari-tv/claude-report"));
    assert_eq!(entry.tokens.total, 1350);
}

#[test]
fn yaml_uses_kebab_case_keys() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.yml");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);
    write_yaml(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(
        body.contains("schema-version:"),
        "expected schema-version key in:\n{}",
        body
    );
    assert!(
        body.contains("session-count:"),
        "expected session-count key in:\n{}",
        body
    );
    assert!(body.contains("jsonl-paths:"), "expected jsonl-paths key in:\n{}", body);
    assert!(body.contains("cache-5m-write:"));
    assert!(body.contains("cache-1h-write:"));
    assert!(body.contains("cache-read:"));
    assert!(!body.contains("schema_version:"));
}

#[test]
fn load_existing_titles_returns_titles_only() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.yml");
    let titled = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("do the thing"));
    let untitled = sample_summary("8b21c34d-1e22-4f5a-b91c-1234567890ab", None);
    write_yaml(
        &path,
        &[titled, untitled],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();

    let titles = load_existing_titles(&path);
    assert_eq!(titles.len(), 1);
    assert_eq!(
        titles.get("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042"),
        Some(&"do the thing".to_string())
    );
}

#[test]
fn load_existing_titles_missing_path_is_empty() {
    let titles = load_existing_titles(Path::new("/nonexistent/cr-test/missing.yml"));
    assert!(titles.is_empty());
}

#[test]
fn write_is_atomic_via_rename() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.yml");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);

    write_yaml(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();
    write_yaml(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();

    let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "no leftover temp files: {:?}", entries);
}
