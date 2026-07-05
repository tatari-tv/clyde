#![allow(clippy::unwrap_used)]

use super::*;
use crate::outcome::{Outcomes, PrRef};
use crate::report::{ModelTokens, Report, SCHEMA_VERSION, SessionEntry};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use tempfile::TempDir;

const SID_SHARED: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_UNIQUE: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

/// A model-token block with a known spend, so re-summed totals are checkable.
fn model_tokens(input: u64, spend: Option<f64>) -> ModelTokens {
    ModelTokens {
        input,
        output: input,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read: 0,
        total: input * 2,
        spend_usd: spend,
    }
}

fn entry_with_outcomes(model: &str, mt: ModelTokens, outcomes: Option<Outcomes>) -> SessionEntry {
    let mut models = BTreeMap::new();
    let session_spend = mt.spend_usd;
    let untracked = if mt.spend_usd.is_none() {
        vec![model.to_string()]
    } else {
        Vec::new()
    };
    models.insert(model.to_string(), mt);
    SessionEntry {
        title: None,
        repo: None,
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        spend_usd: session_spend,
        untracked_models: untracked,
        jsonl_paths: vec![],
        models,
        outcomes,
    }
}

/// Build a one-session report for a host with a single model+spend. Defaults
/// `outcomes-enabled` to `Some(true)` (today's collect always runs extraction, per Phase 4);
/// tests exercising the coverage rules override it after construction.
fn report(host: &str, sid: &str, since: &str, until: &str, model: &str, mt: ModelTokens) -> Report {
    report_with_outcomes(host, sid, since, until, model, mt, None)
}

fn report_with_outcomes(
    host: &str,
    sid: &str,
    since: &str,
    until: &str,
    model: &str,
    mt: ModelTokens,
    outcomes: Option<Outcomes>,
) -> Report {
    let mut sessions = BTreeMap::new();
    let e = entry_with_outcomes(model, mt.clone(), outcomes);
    sessions.insert(sid.to_string(), e);
    let totals = recompute_totals(&sessions, true);
    Report {
        schema_version: SCHEMA_VERSION,
        generated: ts("2026-05-01T00:00:00Z"),
        host: host.to_string(),
        since: ts(since),
        until: ts(until),
        outcomes_enabled: Some(true),
        totals,
        sessions,
    }
}

fn write_report(dir: &std::path::Path, name: &str, r: &Report) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, serde_json::to_string_pretty(r).unwrap()).unwrap();
    path
}

#[test]
fn merge_two_hosts_colliding_id_keeps_both_and_resums_totals() {
    // Two hosts share SID_SHARED with a known spend each; keep-both must preserve both, and
    // totals must be the SUM of both (re-summed), not one overwriting the other.
    let r1 = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.50)),
    );
    let r2 = report(
        "laptop",
        SID_SHARED,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(200, Some(2.50)),
    );

    let merged = merge_reports(vec![r1, r2]).unwrap();

    // Keep-both: both host-prefixed keys survive even though the bare id collided.
    assert_eq!(merged.sessions.len(), 2, "both colliding-id sessions must survive");
    assert!(merged.sessions.contains_key(&format!("desk/{SID_SHARED}")));
    assert!(merged.sessions.contains_key(&format!("laptop/{SID_SHARED}")));

    // Totals re-summed from the merged set, NOT double-counted, NOT one-overwrites-other.
    assert_eq!(merged.totals.sessions, 2);
    assert_eq!(merged.totals.spend_usd, 4.00, "spend must be the sum of both sessions");
    let opus = merged.totals.models.get("claude-opus-4-7").unwrap();
    assert_eq!(opus.input, 300, "token input must be re-summed across both sessions");
    assert_eq!(opus.spend_usd, Some(4.00));

    // Window widened to min(since)/max(until) across inputs.
    assert_eq!(merged.since, ts("2026-04-01T00:00:00Z"));
    assert_eq!(merged.until, ts("2026-04-30T00:00:00Z"));

    // Multi-host marker names both distinct hosts.
    assert_eq!(merged.host, "desk+laptop");
    assert_eq!(merged.schema_version, SCHEMA_VERSION);
}

#[test]
fn merge_distinct_sessions_sums_without_overlap() {
    let r1 = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    let r2 = report(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-sonnet-4-6",
        model_tokens(50, Some(0.25)),
    );

    let merged = merge_reports(vec![r1, r2]).unwrap();
    assert_eq!(merged.sessions.len(), 2);
    assert_eq!(merged.totals.sessions, 2);
    assert_eq!(merged.totals.spend_usd, 1.25);
    assert_eq!(merged.totals.models.len(), 2);
}

#[test]
fn schema_version_mismatch_is_typed_error_naming_both() {
    let mut r1 = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    r1.schema_version = 1;
    let mut r2 = report(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    r2.schema_version = 2;

    // Typed error: match on the variant naming both versions, not a Display-string substring.
    let err = merge_reports(vec![r1, r2]).unwrap_err();
    match err {
        MergeError::SchemaMismatch { expected, found } => {
            assert_eq!(expected, 1);
            assert_eq!(found, 2);
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }
}

#[test]
fn zero_inputs_is_error() {
    let err = read_reports(&[]).unwrap_err();
    assert!(matches!(err, MergeError::NoInputs), "expected NoInputs, got {err:?}");
}

#[test]
fn single_input_is_identity_passthrough() {
    let r = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    // Capture the input's scalar fields and its serialized form BEFORE the move, so we can assert a
    // genuine field-by-field + byte-identical round-trip against the original.
    let original_generated = r.generated;
    let original_host = r.host.clone();
    let original_since = r.since;
    let original_until = r.until;
    let original_schema = r.schema_version;
    let original_spend = r.totals.spend_usd;
    let original_sessions = r.totals.sessions;
    let original_json = serde_json::to_string_pretty(&r).unwrap();

    let merged = merge_reports(vec![r]).unwrap();

    // TRUE identity passthrough: keys are the ORIGINAL bare session ids (NOT re-keyed to
    // "<host>/<id>"), `generated` is the original timestamp (NOT a fresh Utc::now), and host /
    // since / until / totals are all the input's own values, unchanged.
    assert!(
        merged.sessions.contains_key(SID_SHARED),
        "session key must stay the bare id, not be re-keyed"
    );
    assert!(
        !merged.sessions.contains_key(&format!("desk/{SID_SHARED}")),
        "single-input merge must NOT re-key by host"
    );
    assert_eq!(merged.generated, original_generated, "generated must be preserved");
    assert_eq!(merged.host, original_host, "host must be preserved verbatim");
    assert_eq!(merged.since, original_since);
    assert_eq!(merged.until, original_until);
    assert_eq!(merged.schema_version, original_schema);
    assert_eq!(merged.totals.sessions, original_sessions);
    assert_eq!(merged.totals.spend_usd, original_spend);

    // The strongest assertion: the serialized merged report is byte-identical to the input.
    let merged_json = serde_json::to_string_pretty(&merged).unwrap();
    assert_eq!(
        merged_json, original_json,
        "1-input merge must be a byte-identical round-trip"
    );
}

#[test]
fn run_writes_merged_report_to_file() {
    let tmp = TempDir::new().unwrap();
    let r1 = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    let r2 = report(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(200, Some(2.00)),
    );
    let p1 = write_report(tmp.path(), "a.json", &r1);
    let p2 = write_report(tmp.path(), "b.json", &r2);

    let out = tmp.path().join("merged.json");
    let cfg = MergeConfig {
        inputs: vec![p1, p2],
        output: Output::File(out.clone()),
    };
    let result = run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    match result.output {
        OutputDest::File(p) => assert_eq!(p, out),
        other => panic!("expected file output, got {other:?}"),
    }

    // Output round-trips as a valid Report with both sessions and re-summed totals.
    let body = fs::read_to_string(&out).unwrap();
    let parsed: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.totals.sessions, 2);
    assert_eq!(parsed.totals.spend_usd, 3.00);
    assert!(parsed.sessions.contains_key(&format!("desk/{SID_SHARED}")));
    assert!(parsed.sessions.contains_key(&format!("laptop/{SID_UNIQUE}")));
    assert_eq!(parsed.host, "desk+laptop");
}

#[test]
fn run_zero_inputs_errors() {
    let cfg = MergeConfig {
        inputs: vec![],
        output: Output::Stdout,
    };
    assert!(run(&cfg).is_err());
}

fn pr(number: u64, url: &str) -> PrRef {
    PrRef {
        number,
        url: url.to_string(),
        repository: None,
    }
}

#[test]
fn merged_outcomes_totals_are_the_deduped_union_of_both_inputs() {
    // Two hosts' transcripts of overlapping work: a shared PR url (reviewed/babysat from both
    // hosts) must count once in the merged rollup, not twice.
    let shared_pr = "https://github.com/tatari-tv/clyde/pull/10";
    let r1 = report_with_outcomes(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![pr(10, shared_pr)],
            confluence_writes: 1,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 2,
        }),
    );
    let r2 = report_with_outcomes(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(200, Some(2.00)),
        Some(Outcomes {
            commits: vec!["sha-b".to_string()],
            prs: vec![pr(10, shared_pr)],
            confluence_writes: 0,
            jira_writes: 3,
            slack_messages: 0,
            files_edited: 4,
        }),
    );

    let merged = merge_reports(vec![r1, r2]).unwrap();

    assert_eq!(merged.outcomes_enabled, Some(true));
    let outcomes = merged
        .totals
        .outcomes
        .expect("rollup must be present when all inputs enabled");
    assert_eq!(outcomes.sessions_with_commits, 2);
    assert_eq!(outcomes.commits, 2, "sha-a and sha-b are distinct");
    assert_eq!(outcomes.prs_opened, 1, "the shared PR url must count once, not twice");
    assert_eq!(outcomes.confluence_writes, 1);
    assert_eq!(outcomes.jira_writes, 3);
    assert_eq!(outcomes.files_edited, 6, "files-edited is a plain per-session sum");
}

#[test]
fn merge_with_one_input_not_outcomes_enabled_yields_absent_rollup_and_false_flag() {
    let r1 = report_with_outcomes(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
        Some(Outcomes {
            commits: vec!["sha-a".to_string()],
            prs: vec![],
            confluence_writes: 0,
            jira_writes: 0,
            slack_messages: 0,
            files_edited: 1,
        }),
    );
    let mut r2 = report(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(200, Some(2.00)),
    );
    // Simulate a pre-Phase-4 (or --no-outcomes) input: absent flag.
    r2.outcomes_enabled = None;

    let merged = merge_reports(vec![r1, r2]).unwrap();

    assert_eq!(
        merged.outcomes_enabled,
        Some(false),
        "one incapable input must flip the merged flag to false"
    );
    assert!(
        merged.totals.outcomes.is_none(),
        "the rollup must be absent, never a partial count read as complete"
    );
    // The re-summed token/spend totals are unaffected by the outcomes coverage rule.
    assert_eq!(merged.totals.sessions, 2);
    assert_eq!(merged.totals.spend_usd, 3.00);
}

#[test]
fn merge_with_one_input_outcomes_disabled_yields_absent_rollup_and_false_flag() {
    let r1 = report(
        "desk",
        SID_SHARED,
        "2026-04-01T00:00:00Z",
        "2026-04-20T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(100, Some(1.00)),
    );
    let mut r2 = report(
        "laptop",
        SID_UNIQUE,
        "2026-04-05T00:00:00Z",
        "2026-04-30T00:00:00Z",
        "claude-opus-4-7",
        model_tokens(200, Some(2.00)),
    );
    r2.outcomes_enabled = Some(false);

    let merged = merge_reports(vec![r1, r2]).unwrap();

    assert_eq!(merged.outcomes_enabled, Some(false));
    assert!(merged.totals.outcomes.is_none());
}

/// A hand-written v1 (pre-Phase-4) report JSON: no `outcomes-enabled` at the report level, no
/// `outcomes` key on `totals`, no `outcomes` key on the session entry. `#[serde(default)]` must
/// deserialize it cleanly, and it must merge (and, per `render/tests.rs`, render) green.
fn v1_report_json(host: &str, sid: &str) -> String {
    format!(
        r#"{{
            "schema-version": 1,
            "generated": "2026-05-01T00:00:00Z",
            "host": "{host}",
            "since": "2026-04-01T00:00:00Z",
            "until": "2026-04-30T00:00:00Z",
            "totals": {{
                "sessions": 1,
                "spend-usd": 1.0,
                "untracked-models": [],
                "models": {{
                    "claude-opus-4-7": {{
                        "input": 100, "output": 100, "cache-5m-write": 0, "cache-1h-write": 0,
                        "cache-read": 0, "total": 200, "spend-usd": 1.0
                    }}
                }}
            }},
            "sessions": {{
                "{sid}": {{
                    "title": null, "repo": null,
                    "begin": "2026-04-10T10:00:00Z", "end": "2026-04-10T11:00:00Z",
                    "spend-usd": 1.0, "untracked-models": [],
                    "models": {{
                        "claude-opus-4-7": {{
                            "input": 100, "output": 100, "cache-5m-write": 0, "cache-1h-write": 0,
                            "cache-read": 0, "total": 200, "spend-usd": 1.0
                        }}
                    }}
                }}
            }}
        }}"#
    )
}

#[test]
fn v1_report_json_deserializes_with_new_fields_defaulted_absent() {
    let body = v1_report_json("desk", SID_SHARED);
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.outcomes_enabled, None);
    assert!(report.totals.outcomes.is_none());
    let entry = report.sessions.values().next().unwrap();
    assert!(entry.outcomes.is_none());
}

#[test]
fn v1_report_json_merges_green_with_absent_outcomes_enabled() {
    let tmp = TempDir::new().unwrap();
    let p1 = tmp.path().join("desk.json");
    let p2 = tmp.path().join("laptop.json");
    fs::write(&p1, v1_report_json("desk", SID_SHARED)).unwrap();
    fs::write(&p2, v1_report_json("laptop", SID_UNIQUE)).unwrap();
    let out = tmp.path().join("merged.json");

    let cfg = MergeConfig {
        inputs: vec![p1, p2],
        output: Output::File(out.clone()),
    };
    let result = run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);

    let body = fs::read_to_string(&out).unwrap();
    let merged: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(merged.outcomes_enabled, Some(false));
    assert!(merged.totals.outcomes.is_none());
}
