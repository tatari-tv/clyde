//! Schema v12 tests: `scope_version`, the widened `enrich_candidates` predicate it feeds, and the
//! `scope_evidence` read the classifier consults.
//!
//! Their own submodule for the same reason `efficiency.rs` and `activity.rs` are: `db/tests.rs` is near
//! the 1500-line limit and this is a self-contained surface.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use session::ParsedSession;

use crate::db::{Db, EfficiencyWrite};
use crate::export::EnrichStatus;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

/// A candidate row: dormant by activity time so it clears the dormancy filter, with `cwd` under the
/// caller's control (the classifier's first input).
fn parsed(session_id: &str, cwd: &str) -> ParsedSession {
    let long_ago = Utc::now() - Duration::days(30);
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-notes"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("the first prompt".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 5,
        created: Some(long_ago),
        activity_at: Some(long_ago),
        modified: long_ago,
        body: "some body text".to_string(),
        jsonl_paths: vec![PathBuf::from("/tmp/a.jsonl")],
    }
}

fn cutoff() -> DateTime<Utc> {
    Utc::now() - Duration::days(7)
}

fn candidate_ids(db: &Db) -> Vec<String> {
    db.enrich_candidates(Some(cutoff()), 1, 5, false)
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect()
}

/// Write an `outcome_json` carrying repo evidence, the way `reindex_efficiency` does.
fn set_evidence(db: &Db, session_id: &str, repos: &[(&str, u64)], files_edited: u64) {
    let touched: serde_json::Map<String, serde_json::Value> = repos
        .iter()
        .map(|(k, v)| ((*k).to_string(), serde_json::json!(v)))
        .collect();
    let outcome = serde_json::json!({ "repos-touched": touched, "files-edited": files_edited }).to_string();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id,
        efficiency_json: r#"{"session-id":"x","aggregate":{}}"#,
        cache_read_share: None,
        tool_errors: 0,
        cost_usd: 0.0,
        outcome_json: &outcome,
    }])
    .unwrap();
}

/// THE thing Phase 4 exists to do at the SQL layer: a row already recorded `skipped-personal` is
/// re-offered when its `scope_version` is behind the current classifier, and stops being offered once
/// the current version is recorded.
///
/// BITES, and this is the failure mode the design flagged as the obvious way for the phase to be a
/// silent no-op: append the `scope_version` terms as a separate `AND (...)` instead of inside the
/// `skipped-personal` clause and the first assertion returns EMPTY -- the 30 measured rows stay personal
/// forever with the fix invisible.
#[test]
fn a_skipped_personal_row_is_re_offered_when_the_classifier_moves_on() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/notes"), "host-01")
        .unwrap();

    // A pre-v12 skip: recorded personal, no classifier version (the state every existing row is in).
    db.record_enrich_skip(UUID_A, "personal", None, EnrichStatus::SkippedPersonal)
        .unwrap();
    assert_eq!(
        candidate_ids(&db),
        vec![UUID_A.to_string()],
        "a skipped-personal row with NULL scope_version must be re-offered"
    );

    // Once the current version is recorded, the row is settled and drops out.
    db.record_enrich_skip(
        UUID_A,
        "personal",
        Some(session::SCOPE_VERSION),
        EnrichStatus::SkippedPersonal,
    )
    .unwrap();
    assert!(
        candidate_ids(&db).is_empty(),
        "a skip recorded at the CURRENT classifier version is settled"
    );

    // A version behind the current one is re-offered again: the mechanism generalizes to the next bump.
    db.record_enrich_skip(
        UUID_A,
        "personal",
        Some(session::SCOPE_VERSION - 1),
        EnrichStatus::SkippedPersonal,
    )
    .unwrap();
    assert_eq!(
        candidate_ids(&db),
        vec![UUID_A.to_string()],
        "a stale scope_version is re-offered"
    );
}

/// The sibling clause does not undo the fix. `record_enrich_skip` deliberately never touches
/// `enriched_at`, so `enriched_at IS NULL` holds for every `skipped-personal` row and the
/// `prompt_version` clause stays true. Checked explicitly, because it is the OTHER obvious way for the
/// widening to be a silent no-op.
#[test]
fn the_prompt_version_clause_does_not_re_exclude_a_skipped_personal_row() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/notes"), "host-01")
        .unwrap();
    db.record_enrich_skip(UUID_A, "personal", None, EnrichStatus::SkippedPersonal)
        .unwrap();

    let enriched_at: Option<String> = db
        .conn
        .query_row(
            "SELECT enriched_at FROM sessions WHERE session_id = ?1",
            [UUID_A],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        enriched_at, None,
        "record_enrich_skip must leave enriched_at NULL, or the sibling clause would exclude the row"
    );
    assert_eq!(candidate_ids(&db), vec![UUID_A.to_string()]);
}

/// The provisional rule. An evidence-free skip records NO `scope_version`, so the row stays a candidate;
/// an evidence-backed one records it and settles.
///
/// This is the difference between the phase working and being a no-op on exactly the host it exists for:
/// `clyde session enrich` refreshes via `lazy_reindex`, which never runs `reindex_efficiency`, the sole
/// writer of `outcome_json`. On a catalog that has never had a full explicit reindex, EVERY touch set is
/// empty.
///
/// BITES: record `Some(SCOPE_VERSION)` unconditionally in `sessions::enrich` and the first assertion
/// here still passes (it calls the DB directly) but the row would never be reconsidered in the real
/// pass; the second assertion is what pins the two apart.
#[test]
fn an_evidence_free_skip_leaves_scope_version_null_and_stays_a_candidate() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/notes"), "host-01")
        .unwrap();
    db.upsert_session(&parsed(UUID_B, "/home/saidler/notes"), "host-01")
        .unwrap();

    // A: no evidence -> provisional, NULL version.
    db.record_enrich_skip(UUID_A, "personal", None, EnrichStatus::SkippedPersonal)
        .unwrap();
    // B: evidence in hand -> settled at the current version.
    db.record_enrich_skip(
        UUID_B,
        "personal",
        Some(session::SCOPE_VERSION),
        EnrichStatus::SkippedPersonal,
    )
    .unwrap();

    let stored = |id: &str| -> Option<i64> {
        db.conn
            .query_row("SELECT scope_version FROM sessions WHERE session_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(stored(UUID_A), None, "a provisional decision records no version");
    assert_eq!(stored(UUID_B), Some(session::SCOPE_VERSION));
    assert_eq!(
        candidate_ids(&db),
        vec![UUID_A.to_string()],
        "only the provisional row is still a candidate"
    );
}

/// `scope_evidence` reads BOTH values from ONE parse of the same `outcome_json`, and degrades to
/// all-empty (never an error) on a missing or malformed blob -- which makes the classifier fall through
/// to personal, the fail-safe direction.
#[test]
fn scope_evidence_reads_both_values_from_one_blob_and_degrades_safely() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/notes"), "host-01")
        .unwrap();

    // No outcome_json yet: the state every row is in before a full reindex.
    let empty = db.scope_evidence(UUID_A).unwrap();
    assert!(empty.repos_touched.is_empty());
    assert_eq!(empty.files_edited, 0);

    set_evidence(&db, UUID_A, &[("tatari-tv/philo", 2), ("tatari-tv/clyde", 1)], 3);
    let evidence = db.scope_evidence(UUID_A).unwrap();
    assert_eq!(evidence.repos_touched.len(), 2);
    assert_eq!(evidence.repos_touched["tatari-tv/philo"], 2);
    assert_eq!(evidence.files_edited, 3, "files-edited comes from the same parse");
    assert_eq!(
        evidence.repos_touched.values().sum::<u64>(),
        evidence.files_edited,
        "this row is fully accounted for, so the classifier's totality check passes"
    );

    // A malformed blob is a warning, not an error, and yields the fail-safe empty answer.
    db.conn
        .execute(
            "UPDATE sessions SET outcome_json = '{not json' WHERE session_id = ?1",
            [UUID_A],
        )
        .unwrap();
    let broken = db.scope_evidence(UUID_A).unwrap();
    assert_eq!(broken, crate::db::ScopeEvidence::default());

    // An absent session is not an error either.
    assert_eq!(
        db.scope_evidence("00000000-0000-4000-8000-000000000000").unwrap(),
        crate::db::ScopeEvidence::default()
    );
}

/// The migration: `scope_version` exists, and it is its OWN step rather than an addition to v11's.
///
/// The second half is the point. A DB already at v11 (the exact state a host that ran Phase 3 alone is
/// in) still gains the column, because `migrate` returns early only when `user_version >=
/// SCHEMA_VERSION` and v12 raised that bar. Had `scope_version` been appended to `migrate_v11_activity`,
/// this DB would skip the ladder entirely and every query naming the column would fail.
#[test]
fn a_v11_db_gains_scope_version_because_v12_is_its_own_step() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("sessions.db");
    {
        let db = Db::open_at(&path).unwrap();
        db.upsert_session(&parsed(UUID_A, "/home/saidler/notes"), "host-01")
            .unwrap();
    }
    // Rewind to v11: a host that landed Phase 3 and not Phase 4.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 11i64).unwrap();
    }

    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert!(uv >= 12, "reopen migrates to at least v12, got {uv}");
    assert!(
        PathBuf::from(format!("{}.pre-v12.bak", path.display())).exists(),
        "a genuine pre-v12 catalog is snapshotted first"
    );

    let cols: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(sessions)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(cols.contains(&"scope_version".to_string()), "columns: {cols:?}");

    // And the column is writable/readable through the real path on this migrated DB.
    db.record_enrich_skip(
        UUID_A,
        "personal",
        Some(session::SCOPE_VERSION),
        EnrichStatus::SkippedPersonal,
    )
    .unwrap();
    let stored: Option<i64> = db
        .conn
        .query_row(
            "SELECT scope_version FROM sessions WHERE session_id = ?1",
            [UUID_A],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(session::SCOPE_VERSION));
}
