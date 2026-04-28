#![allow(clippy::unwrap_used)]

use super::*;
use crate::session::{SessionSummary, TokenTotals};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn opus_totals() -> TokenTotals {
    let mut t = TokenTotals::default();
    t.add(&crate::parse::TokenUsage {
        input_tokens: 100,
        output_tokens: 200,
        cache_5m_write_tokens: 50,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 1000,
    });
    t
}

fn sample_summary(sid: &str, title: Option<&str>) -> SessionSummary {
    let mut models = BTreeMap::new();
    models.insert("claude-opus-4-7".to_string(), opus_totals());
    SessionSummary {
        session_id: sid.into(),
        repo: Some("tatari-tv/claude-report".into()),
        cwd: Some(PathBuf::from("/home/u/r")),
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        models,
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
    assert_eq!(report.totals.sessions, 1);
    assert!(report.totals.spend_usd > 0.0);
    let entry = &report.sessions["9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042"];
    assert_eq!(entry.title.as_deref(), Some("do the thing"));
    assert_eq!(entry.repo.as_deref(), Some("tatari-tv/claude-report"));
    let opus = entry.models.get("claude-opus-4-7").unwrap();
    assert_eq!(opus.input, 100);
    assert_eq!(opus.output, 200);
    assert!(opus.spend_usd > 0.0);
}

#[test]
fn yaml_uses_kebab_case_keys_and_no_jsonl_paths() {
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
    assert!(body.contains("schema-version:"), "body:\n{}", body);
    assert!(body.contains("totals:"), "body:\n{}", body);
    assert!(body.contains("spend-usd:"), "body:\n{}", body);
    assert!(body.contains("cache-5m-write:"));
    assert!(body.contains("cache-1h-write:"));
    assert!(body.contains("cache-read:"));
    assert!(!body.contains("schema_version:"));
    assert!(!body.contains("jsonl-paths:"), "jsonl-paths must not appear: {}", body);
}

#[test]
fn title_appears_first_in_session_entry() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.yml");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("titled"));
    write_yaml(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    let session_idx = body.find("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042:").unwrap();
    let title_idx = body[session_idx..].find("title:").unwrap();
    let repo_idx = body[session_idx..].find("repo:").unwrap();
    assert!(
        title_idx < repo_idx,
        "title must appear before repo:\n{}",
        &body[session_idx..]
    );
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
