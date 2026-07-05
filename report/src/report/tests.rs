#![allow(clippy::unwrap_used)]

use super::*;
use crate::session::{SessionSummary, TokenTotals};
use claude_pricing::TokenUsage;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
}

fn opus_totals() -> TokenTotals {
    let mut t = TokenTotals::default();
    t.add(&TokenUsage {
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
        outcomes: None,
    }
}

#[test]
fn write_json_round_trips() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("do the thing"));
    let count = write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();
    assert_eq!(count, 1);

    let body = fs::read_to_string(&path).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
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
    assert!(opus.spend_usd.unwrap() > 0.0);
    assert!(entry.untracked_models.is_empty());
    assert_eq!(entry.jsonl_paths, vec![PathBuf::from("/path/to/parent.jsonl")]);
    assert!(entry.spend_usd.unwrap() > 0.0);
}

#[test]
fn json_uses_kebab_case_keys_and_emits_jsonl_paths() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"schema-version\":"), "body:\n{}", body);
    assert!(body.contains("\"totals\":"), "body:\n{}", body);
    assert!(body.contains("\"spend-usd\":"), "body:\n{}", body);
    assert!(body.contains("\"cache-5m-write\":"));
    assert!(body.contains("\"cache-1h-write\":"));
    assert!(body.contains("\"cache-read\":"));
    assert!(
        body.contains("\"untracked-models\":"),
        "untracked-models must appear: {}",
        body
    );
    assert!(body.contains("\"jsonl-paths\":"), "jsonl-paths must appear: {}", body);
    assert!(!body.contains("\"schema_version\":"));
}

#[test]
fn title_appears_first_in_session_entry() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("titled"));
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    let session_idx = body.find("\"9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042\":").unwrap();
    let tail = body.get(session_idx..).unwrap();
    let title_idx = tail.find("\"title\":").unwrap();
    let repo_idx = tail.find("\"repo\":").unwrap();
    assert!(title_idx < repo_idx, "title must appear before repo:\n{}", tail);
}

#[test]
fn load_existing_titles_returns_titles_only() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let titled = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some("do the thing"));
    let untitled = sample_summary("8b21c34d-1e22-4f5a-b91c-1234567890ab", None);
    write_json(
        &path,
        &[titled, untitled],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
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
    let titles = load_existing_titles(Path::new("/nonexistent/cr-test/missing.json"));
    assert!(titles.is_empty());
}

fn small_totals(input: u64) -> TokenTotals {
    let mut t = TokenTotals::default();
    t.add(&TokenUsage {
        input_tokens: input,
        output_tokens: 0,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    });
    t
}

fn summary_with_models(sid: &str, models: BTreeMap<String, TokenTotals>) -> SessionSummary {
    SessionSummary {
        session_id: sid.into(),
        repo: None,
        cwd: None,
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        models,
        jsonl_paths: vec![],
        title: None,
        outcomes: None,
    }
}

#[test]
fn all_priced_session_has_some_spend_and_no_untracked() {
    let mut models = BTreeMap::new();
    models.insert("claude-opus-4-7".into(), opus_totals());
    let s = summary_with_models("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", models);
    let entry = to_entry(&s, &pricing());
    assert!(entry.spend_usd.is_some());
    assert!(entry.spend_usd.unwrap() > 0.0);
    assert!(entry.untracked_models.is_empty());
}

#[test]
fn all_untracked_session_has_none_spend_and_lists_models() {
    let mut models = BTreeMap::new();
    models.insert("not-a-real-model".into(), small_totals(1_000_000));
    let s = summary_with_models("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", models);
    let entry = to_entry(&s, &pricing());
    assert_eq!(entry.spend_usd, None);
    assert_eq!(entry.untracked_models, vec!["not-a-real-model".to_string()]);
}

#[test]
fn mixed_session_has_partial_spend_and_flags_untracked() {
    let mut models = BTreeMap::new();
    models.insert("claude-opus-4-7".into(), opus_totals());
    models.insert("not-a-real-model".into(), small_totals(1_000_000));
    let s = summary_with_models("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", models);
    let entry = to_entry(&s, &pricing());
    assert!(entry.spend_usd.is_some(), "mixed session must report partial spend");
    assert!(entry.spend_usd.unwrap() > 0.0);
    assert_eq!(
        entry.untracked_models,
        vec!["not-a-real-model".to_string()],
        "mixed session must flag the untracked model"
    );
}

#[test]
fn totals_untracked_models_dedupes_across_sessions() {
    let mut s1_models = BTreeMap::new();
    s1_models.insert("ghost-model".into(), small_totals(10));
    let s1 = summary_with_models("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", s1_models);
    let mut s2_models = BTreeMap::new();
    s2_models.insert("ghost-model".into(), small_totals(20));
    s2_models.insert("phantom-model".into(), small_totals(30));
    let s2 = summary_with_models("8b21c34d-1e22-4f5a-b91c-1234567890ab", s2_models);

    let report = build_report(
        &[s1, s2],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    );
    assert_eq!(
        report.totals.untracked_models,
        vec!["ghost-model".to_string(), "phantom-model".to_string()]
    );
}

#[test]
fn json_with_null_spend_round_trips_to_none() {
    let mut models = BTreeMap::new();
    models.insert("not-a-real-model".into(), small_totals(1_000_000));
    let s = summary_with_models("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", models);
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"spend-usd\": null"), "body:\n{}", body);
    let parsed: Report = serde_json::from_str(&body).unwrap();
    let entry = parsed.sessions.values().next().unwrap();
    assert_eq!(entry.spend_usd, None);
    let mt = entry.models.get("not-a-real-model").unwrap();
    assert_eq!(mt.spend_usd, None);
}

#[test]
fn write_is_atomic_via_rename() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = sample_summary("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);

    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
    )
    .unwrap();

    let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "no leftover temp files: {:?}", entries);
}
