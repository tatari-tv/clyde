#![allow(clippy::unwrap_used)]

use super::*;
use crate::outcome::{Outcomes, PrRef};
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
        true,
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
        true,
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
        true,
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
        true,
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
        true,
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
        true,
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

fn pr(number: u64, url: &str) -> PrRef {
    PrRef {
        number,
        url: url.to_string(),
        repository: None,
    }
}

fn summary_with_outcomes(sid: &str, outcomes: Option<Outcomes>) -> SessionSummary {
    let mut models = BTreeMap::new();
    models.insert("claude-opus-4-7".to_string(), opus_totals());
    SessionSummary {
        session_id: sid.into(),
        repo: None,
        cwd: None,
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        models,
        jsonl_paths: vec![],
        title: None,
        outcomes,
    }
}

#[test]
fn to_entry_carries_outcomes_through_untouched() {
    let outcomes = Outcomes {
        commits: vec!["sha-a".to_string()],
        prs: vec![pr(1, "https://github.com/tatari-tv/clyde/pull/1")],
        confluence_writes: 1,
        jira_writes: 0,
        slack_messages: 2,
        files_edited: 3,
    };
    let s = summary_with_outcomes("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", Some(outcomes.clone()));
    let entry = to_entry(&s, &pricing());
    assert_eq!(entry.outcomes, Some(outcomes));
}

#[test]
fn to_entry_absent_outcomes_stays_none() {
    let s = summary_with_outcomes("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);
    let entry = to_entry(&s, &pricing());
    assert_eq!(entry.outcomes, None);
}

#[test]
fn build_report_rolls_up_outcomes_with_global_dedupe_and_marks_enabled() {
    let shared_pr = "https://github.com/tatari-tv/clyde/pull/10";
    let s1 = summary_with_outcomes(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042",
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![pr(10, shared_pr)],
            confluence_writes: 1,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 2,
        }),
    );
    let s2 = summary_with_outcomes(
        "8b21c34d-1e22-4f5a-b91c-1234567890ab",
        Some(Outcomes {
            // Shares "sha-a" with s1 (e.g. cherry-picked into both session groups) and adds one
            // new sha; shares the same PR url too — both must dedupe GLOBALLY, not just locally.
            commits: vec!["sha-a".to_string(), "sha-b".to_string()],
            prs: vec![pr(10, shared_pr)],
            confluence_writes: 0,
            jira_writes: 4,
            slack_messages: 0,
            files_edited: 3,
        }),
    );
    // A third session with no outcomes at all must not affect the rollup or its coverage flag.
    let s3 = summary_with_outcomes("no-outcomes-session", None);

    let report = build_report(
        &[s1, s2, s3],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
    );

    assert_eq!(report.outcomes_enabled, Some(true));
    let outcomes = report.totals.outcomes.expect("rollup must be present");
    assert_eq!(outcomes.sessions_with_commits, 2);
    assert_eq!(outcomes.commits, 2, "sha-a/sha-b distinct across both sessions");
    assert_eq!(outcomes.prs_opened, 1, "shared PR url counts once, globally");
    assert_eq!(outcomes.confluence_writes, 1);
    assert_eq!(outcomes.jira_writes, 4);
    assert_eq!(outcomes.files_edited, 5, "files-edited is a plain per-session sum");
}

#[test]
fn build_report_with_no_outcomes_observed_still_enables_and_yields_zeroed_rollup() {
    let s = summary_with_outcomes("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", None);
    let report = build_report(
        &[s],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
    );
    assert_eq!(report.outcomes_enabled, Some(true));
    let outcomes = report.totals.outcomes.expect("rollup present even when all-zero");
    assert_eq!(outcomes, crate::outcome::OutcomeTotals::default());
}

#[test]
fn write_json_round_trips_session_outcomes() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = summary_with_outcomes(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042",
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![],
            confluence_writes: 0,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 1,
        }),
    );
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"outcomes-enabled\": true"), "body:\n{}", body);
    let report: Report = serde_json::from_str(&body).unwrap();
    let entry = &report.sessions["9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042"];
    assert_eq!(entry.outcomes.as_ref().unwrap().commits, vec!["sha-a".to_string()]);
    assert_eq!(report.totals.outcomes.unwrap().commits, 1);
}

/// A hand-written v1 (pre-Phase-4) report JSON with none of the new fields. `#[serde(default)]`
/// must deserialize it cleanly (backward compat is a tested criterion, D2).
#[test]
fn v1_report_json_without_outcomes_fields_deserializes_cleanly() {
    let body = r#"{
        "schema-version": 1,
        "generated": "2026-05-01T00:00:00Z",
        "host": "desk",
        "since": "2026-04-01T00:00:00Z",
        "until": "2026-04-30T00:00:00Z",
        "totals": {
            "sessions": 1,
            "spend-usd": 1.0,
            "untracked-models": [],
            "models": {}
        },
        "sessions": {
            "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042": {
                "title": null, "repo": null,
                "begin": "2026-04-10T10:00:00Z", "end": "2026-04-10T11:00:00Z",
                "spend-usd": null, "untracked-models": [],
                "models": {}
            }
        }
    }"#;
    let report: Report = serde_json::from_str(body).unwrap();
    assert_eq!(report.outcomes_enabled, None);
    assert!(report.totals.outcomes.is_none());
    let entry = report.sessions.values().next().unwrap();
    assert!(entry.outcomes.is_none());
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
        true,
    )
    .unwrap();
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
    )
    .unwrap();

    let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "no leftover temp files: {:?}", entries);
}

/// Phase 5 (`--no-outcomes`): `build_report(.., outcomes_enabled: false)` must yield
/// `outcomes-enabled: Some(false)` and NO `outcomes` field on totals, even when a summary
/// happens to carry outcome data (fail closed at the persist seam, not just the extract seam).
#[test]
fn build_report_with_outcomes_disabled_yields_false_flag_and_absent_totals_rollup() {
    let s = summary_with_outcomes(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042",
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![],
            confluence_writes: 0,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 1,
        }),
    );
    let report = build_report(
        &[s],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        false,
    );
    assert_eq!(report.outcomes_enabled, Some(false));
    assert!(
        report.totals.outcomes.is_none(),
        "totals.outcomes must be absent, not zeroed"
    );
}

/// A stray per-session `outcomes` value must never survive onto the persisted `SessionEntry`
/// when the report as a whole is `outcomes_enabled: false` -- the design's "no outcomes fields
/// on sessions/totals" contract applies to sessions too, not only the totals rollup.
#[test]
fn build_report_with_outcomes_disabled_strips_session_outcomes() {
    let s = summary_with_outcomes(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042",
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![],
            confluence_writes: 0,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 1,
        }),
    );
    let report = build_report(
        &[s],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        false,
    );
    let entry = report.sessions.values().next().unwrap();
    assert!(entry.outcomes.is_none());
}

#[test]
fn write_json_with_outcomes_disabled_emits_no_outcomes_keys() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = summary_with_outcomes(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042",
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![],
            confluence_writes: 0,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 1,
        }),
    );
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        false,
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"outcomes-enabled\": false"), "body:\n{}", body);
    assert!(!body.contains("\"outcomes\":"), "no outcomes key anywhere: {}", body);
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.outcomes_enabled, Some(false));
    assert!(report.totals.outcomes.is_none());
    let entry = report.sessions.values().next().unwrap();
    assert!(entry.outcomes.is_none());
}
