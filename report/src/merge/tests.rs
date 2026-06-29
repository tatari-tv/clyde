#![allow(clippy::unwrap_used)]

use super::*;
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

fn entry(model: &str, mt: ModelTokens) -> SessionEntry {
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
    }
}

/// Build a one-session report for a host with a single model+spend.
fn report(host: &str, sid: &str, since: &str, until: &str, model: &str, mt: ModelTokens) -> Report {
    let mut sessions = BTreeMap::new();
    let e = entry(model, mt.clone());
    sessions.insert(sid.to_string(), e);
    let totals = recompute_totals(&sessions);
    Report {
        schema_version: SCHEMA_VERSION,
        generated: ts("2026-05-01T00:00:00Z"),
        host: host.to_string(),
        since: ts(since),
        until: ts(until),
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
        OutputDest::Stdout => panic!("expected file output"),
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
