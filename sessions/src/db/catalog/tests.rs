#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use session::ParsedSession;

use crate::db::{Db, EfficiencyWrite};
use crate::model::Filters;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
const UUID_C: &str = "7c19b25e-0d11-4e4b-a82d-2345678901bc";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A minimal `ParsedSession` with an explicit `modified` (drives the window filter). `created` is
/// fixed at 2026-06-01, so a session "created in June, modified in July" (the Phase 0 boundary
/// fixture) is built by passing a July `modified`.
fn parsed(session_id: &str, transcript: &str, modified: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/clyde")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-clyde"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("the first prompt".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 5,
        created: Some(dt("2026-06-01T00:00:00Z")),
        modified: dt(modified),
        body: "some body text".to_string(),
        jsonl_paths: vec![PathBuf::from(transcript)],
    }
}

/// Success criterion: "Returns N sessions for a `since`/`until` window with efficiency + outcomes
/// attached in ONE call." Two sessions inside the window, each carrying distinct efficiency +
/// outcome blobs, both must come back from the single `catalog` call.
#[test]
fn catalog_returns_window_with_efficiency_and_outcomes_attached() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "2026-06-15T10:00:00Z"), "desk")
        .unwrap();
    db.upsert_session(&parsed(UUID_B, "/tmp/b.jsonl", "2026-06-20T10:00:00Z"), "desk")
        .unwrap();
    db.set_efficiency_many(&[
        EfficiencyWrite {
            session_id: UUID_A,
            efficiency_json: r#"{"aggregate":{"cache-read-share":0.5,"raw":{"tool-errors":1,"cost-usd":1.25}}}"#,
            cache_read_share: Some(0.5),
            tool_errors: 1,
            cost_usd: 1.25,
            outcome_json: r#"{"commits":["abc123"],"prs":[],"confluence-writes":0,"jira-writes":0,"slack-messages":0,"files-edited":2}"#,
        },
        EfficiencyWrite {
            session_id: UUID_B,
            efficiency_json: r#"{"aggregate":{"cache-read-share":0.8,"raw":{"tool-errors":0,"cost-usd":0.5}}}"#,
            cache_read_share: Some(0.8),
            tool_errors: 0,
            cost_usd: 0.5,
            outcome_json: "{}",
        },
    ])
    .unwrap();

    let entries = db
        .catalog(&Filters {
            since: Some(dt("2026-06-01T00:00:00Z")),
            until: Some(dt("2026-06-30T23:59:59Z")),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(
        entries.len(),
        2,
        "both sessions fall inside the window, one call returns both"
    );
    let a = entries.iter().find(|e| e.record.session_id == UUID_A).unwrap();
    assert_eq!(a.cache_read_share, Some(0.5));
    assert_eq!(a.tool_errors, Some(1));
    assert_eq!(a.cost_usd, Some(1.25));
    assert!(a.efficiency_json.as_deref().unwrap().contains("cache-read-share"));
    assert!(a.outcome_json.as_deref().unwrap().contains("abc123"));
    let b = entries.iter().find(|e| e.record.session_id == UUID_B).unwrap();
    assert_eq!(b.cache_read_share, Some(0.8));
    assert_eq!(b.outcome_json.as_deref(), Some("{}"));
}

/// Success criterion: "The `until` bound EXCLUDES a session modified after `until` (assert on a
/// fixture spanning the boundary)." Fixture matches the Phase 0 spike's live measurement: a session
/// created in June but modified in July. BITES: drop the `until` clause from `append_filters` and
/// both sessions come back, failing the `len() == 1` assertion.
#[test]
fn catalog_until_bound_excludes_session_modified_after_until() {
    let db = Db::open_memory().unwrap();
    // Inside the window: created + modified in June.
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "2026-06-28T10:00:00Z"), "desk")
        .unwrap();
    // The Phase 0 boundary fixture: created in June, modified in July -- straddles a June/July
    // `until` cut exactly like the 21 real sessions the spike measured.
    db.upsert_session(&parsed(UUID_B, "/tmp/b.jsonl", "2026-07-02T10:00:00Z"), "desk")
        .unwrap();

    let until_june = dt("2026-06-30T23:59:59Z");
    let entries = db
        .catalog(&Filters {
            until: Some(until_june),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the July-modified session must be excluded by the June `until` bound"
    );
    assert_eq!(entries[0].record.session_id, UUID_A);

    // Proves the bound (not the fixture) is what excludes B: with no `until`, both come back.
    let unbounded = db.catalog(&Filters::default()).unwrap();
    assert_eq!(unbounded.len(), 2, "no until bound -> both sessions returned");

    // Inclusive upper bound: a session modified EXACTLY at `until` is included, mirroring `since`'s
    // inclusive lower bound (`since <= s.modified <= until`).
    db.upsert_session(&parsed(UUID_C, "/tmp/c.jsonl", "2026-06-30T23:59:59Z"), "desk")
        .unwrap();
    let with_boundary = db
        .catalog(&Filters {
            until: Some(until_june),
            ..Default::default()
        })
        .unwrap();
    assert!(
        with_boundary.iter().any(|e| e.record.session_id == UUID_C),
        "modified == until must be included (inclusive bound)"
    );
}

/// Success criterion: "Single-session `get_efficiency_json` parity on a spot-checked session (same
/// bytes)." The bulk read and the single-session read pull the same stored columns, so both the
/// efficiency and outcome blobs must be byte-identical.
#[test]
fn catalog_json_blobs_are_byte_identical_to_single_session_reads() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "2026-06-15T10:00:00Z"), "desk")
        .unwrap();
    let efficiency_blob = r#"{"aggregate":{"cache-read-share":0.42,"raw":{"tool-errors":3,"cost-usd":9.99}}}"#;
    let outcome_blob = r#"{"commits":["deadbeef"],"prs":[],"confluence-writes":1,"jira-writes":0,"slack-messages":2,"files-edited":4}"#;
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: efficiency_blob,
        cache_read_share: Some(0.42),
        tool_errors: 3,
        cost_usd: 9.99,
        outcome_json: outcome_blob,
    }])
    .unwrap();

    let entries = db.catalog(&Filters::default()).unwrap();
    let entry = entries.iter().find(|e| e.record.session_id == UUID_A).unwrap();

    let single_efficiency = db.get_efficiency_json(UUID_A).unwrap();
    let single_outcome = db.get_outcome_json(UUID_A).unwrap();

    assert_eq!(
        entry.efficiency_json, single_efficiency,
        "bulk-read efficiency_json must be byte-identical to the single-session read"
    );
    assert_eq!(
        entry.outcome_json, single_outcome,
        "bulk-read outcome_json must be byte-identical to the single-session read"
    );
    assert_eq!(entry.efficiency_json.as_deref(), Some(efficiency_blob));
    assert_eq!(entry.outcome_json.as_deref(), Some(outcome_blob));
}

/// An un-reindexed session (never passed through `set_efficiency_many`) carries `None` on every
/// efficiency/outcome/scalar field, never a fabricated zero -- the fail-closed distinction Phase 4
/// depends on to tell "no data yet" from "computed zero".
#[test]
fn catalog_un_reindexed_session_has_none_efficiency_and_outcome() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "2026-06-15T10:00:00Z"), "desk")
        .unwrap();

    let entries = db.catalog(&Filters::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].efficiency_json, None);
    assert_eq!(entries[0].outcome_json, None);
    assert_eq!(entries[0].cache_read_share, None);
    assert_eq!(entries[0].tool_errors, None);
    assert_eq!(entries[0].cost_usd, None);
}
