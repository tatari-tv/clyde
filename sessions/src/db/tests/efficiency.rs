#![allow(clippy::unwrap_used)]

//! Phase 6: schema v6 efficiency annotation (efficiency_json + indexed scalars). Split from
//! `db/tests.rs` to keep that file under the line-count limit. Parent-module test helpers
//! (`parsed`, `dt`, `UUID_*`, `revision_counter`, `updated_at_of`) come in via `use super::*`;
//! `db` items (`Db`, `EfficiencyWrite`, and the private `SCHEMA_VERSION`/`V5_TRIGGERS_SQL`) are
//! reachable because this is a descendant module of `db`.

use std::path::Path;

use super::*;

/// The stored efficiency columns for one session: (efficiency_json, cache_read_share, tool_errors,
/// cost_usd). All `Option` since a fresh/un-annotated row leaves every column `NULL`.
fn efficiency_of(db: &Db, session_id: &str) -> (Option<String>, Option<f64>, Option<i64>, Option<f64>) {
    db.conn
        .query_row(
            "SELECT efficiency_json, cache_read_share, tool_errors, cost_usd FROM sessions WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
}

/// `set_efficiency_many` stores all four columns, keeps the indexed scalars in lock step with the
/// stored JSON, and — the load-bearing invariant — does NOT advance `updated_at` (efficiency is a
/// derived read-side annotation, not a content change). BITES: drop the trigger suppression in
/// `set_efficiency_many` and the cursor advances, failing the `updated_at`/counter assertions.
#[test]
fn v6_set_efficiency_stores_columns_without_advancing_updated_at() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap(); // revision 1

    // A fresh row starts fully un-annotated.
    assert_eq!(efficiency_of(&db, UUID_A), (None, None, None, None));

    let counter_before = revision_counter(&db);
    let updated_at_before = updated_at_of(&db, UUID_A);

    // The JSON carries the SAME scalar values passed alongside it (the single-computation-path shape
    // the efficiency crate produces): 0.5 / 4 / 2.5.
    let blob = r#"{"aggregate":{"cache-read-share":0.5,"raw":{"tool-errors":4,"cost-usd":2.5}}}"#;
    let written = db
        .set_efficiency_many(&[EfficiencyWrite {
            session_id: UUID_A,
            efficiency_json: blob,
            cache_read_share: Some(0.5),
            tool_errors: 4,
            cost_usd: 2.5,
        }])
        .unwrap();
    assert_eq!(written, 1, "one row annotated");

    // Columns stored verbatim.
    let (json, share, errors, cost) = efficiency_of(&db, UUID_A);
    assert_eq!(json.as_deref(), Some(blob), "efficiency_json stored verbatim");
    assert_eq!(share, Some(0.5));
    assert_eq!(errors, Some(4));
    assert_eq!(cost, Some(2.5));

    // Storage consistency: the indexed scalars equal the values parsed back out of the stored JSON,
    // so an index query and a JSON parse can never disagree.
    let parsed_json: serde_json::Value = serde_json::from_str(json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_json["aggregate"]["cache-read-share"].as_f64(), share);
    assert_eq!(parsed_json["aggregate"]["raw"]["tool-errors"].as_i64(), errors);
    assert_eq!(parsed_json["aggregate"]["raw"]["cost-usd"].as_f64(), cost);

    // The cursor did NOT move: neither the row's revision nor the counter.
    assert_eq!(
        updated_at_of(&db, UUID_A),
        updated_at_before,
        "writing efficiency must NOT advance the row's updated_at revision"
    );
    assert_eq!(
        revision_counter(&db),
        counter_before,
        "writing efficiency must NOT advance the export_meta counter"
    );

    // The suppression is scoped to the batch: a subsequent CONTENT write still advances normally
    // (the trigger was restored).
    assert!(db.record_enrich_failure(UUID_A, "work", "boom").unwrap());
    assert_eq!(
        revision_counter(&db),
        counter_before + 1,
        "a content write after the efficiency batch advances the cursor (trigger restored)"
    );
}

/// A `None` cache-read-share (a zero-token scope) round-trips as a stored `NULL`, never `0.0`.
#[test]
fn v6_set_efficiency_none_share_stores_null() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: r#"{"aggregate":{"cache-read-share":null}}"#,
        cache_read_share: None,
        tool_errors: 0,
        cost_usd: 0.0,
    }])
    .unwrap();
    let (_, share, errors, cost) = efficiency_of(&db, UUID_A);
    assert_eq!(share, None, "None share stores as SQL NULL, not 0.0");
    assert_eq!(errors, Some(0));
    assert_eq!(cost, Some(0.0));
}

/// A content re-upsert (grown transcript) INVALIDATES a stale efficiency annotation by NULLing it,
/// so the next `efficiency IS NULL` reindex recomputes against the new transcript. The invalidation
/// rides the content UPDATE's own cursor bump (a legitimate content change).
#[test]
fn v6_content_update_nulls_stale_efficiency() {
    let db = Db::open_memory().unwrap();
    let mut p = parsed(UUID_A, "/tmp/a.jsonl");
    db.upsert_session(&p, "desk").unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: r#"{"aggregate":{}}"#,
        cache_read_share: Some(0.9),
        tool_errors: 1,
        cost_usd: 0.5,
    }])
    .unwrap();
    assert!(
        efficiency_of(&db, UUID_A).0.is_some(),
        "annotated before the content change"
    );
    assert!(
        db.sessions_missing_efficiency().unwrap().is_empty(),
        "an annotated row is not a backfill candidate"
    );

    // Grow the transcript (newer mtime) -> content UPDATE -> efficiency nulled.
    p.modified = dt("2026-06-25T10:00:00Z");
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::Updated);
    assert_eq!(
        efficiency_of(&db, UUID_A),
        (None, None, None, None),
        "a content change must invalidate the stale efficiency annotation"
    );
    assert_eq!(
        db.sessions_missing_efficiency().unwrap(),
        vec![UUID_A.to_string()],
        "the grown session becomes a backfill candidate again"
    );
}

/// `sessions_missing_efficiency` returns non-archived un-annotated rows only: an annotated row and an
/// archived row are both excluded.
#[test]
fn v6_sessions_missing_efficiency_excludes_annotated_and_archived() {
    let tmp = tempfile::TempDir::new().unwrap();
    let live_a = tmp.path().join("a.jsonl");
    let live_b = tmp.path().join("b.jsonl");
    std::fs::write(&live_a, "{}").unwrap();
    std::fs::write(&live_b, "{}").unwrap();

    let db = Db::open_memory().unwrap();
    // A: un-annotated, live (real transcript on disk) -> a candidate.
    db.upsert_session(&parsed(UUID_A, live_a.to_str().unwrap()), "desk")
        .unwrap();
    // B: annotated, live -> excluded.
    db.upsert_session(&parsed(UUID_B, live_b.to_str().unwrap()), "desk")
        .unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_B,
        efficiency_json: r#"{"aggregate":{}}"#,
        cache_read_share: None,
        tool_errors: 0,
        cost_usd: 0.0,
    }])
    .unwrap();
    // C: un-annotated but archived (reaped transcript) -> excluded (nothing to recompute from).
    db.upsert_session(&parsed(UUID_C, "/tmp/reaped.jsonl"), "desk").unwrap();
    db.reconcile_archived().unwrap();

    assert_eq!(
        db.sessions_missing_efficiency().unwrap(),
        vec![UUID_A.to_string()],
        "only the live, un-annotated session is a backfill candidate"
    );
}

/// The exact `sessions` schema clyde shipped at v5: the v4 columns PLUS `updated_at`, WITHOUT the v6
/// efficiency columns. Used to build a real v5 DB so the v5 -> v6 migration path is exercised end to
/// end — and, critically, so the v5 revision backfill does NOT re-run and rewind live cursors.
const V5_SESSIONS_SQL: &str = "\
CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT NOT NULL UNIQUE,
    cwd             TEXT,
    project_dir     TEXT NOT NULL,
    transcript_path TEXT NOT NULL,
    title           TEXT,
    first_prompt    TEXT,
    summary         TEXT,
    tags            TEXT NOT NULL DEFAULT '',
    git_branch      TEXT,
    model           TEXT,
    n_msgs          INTEGER NOT NULL DEFAULT 0,
    created         TEXT,
    modified        TEXT NOT NULL,
    cost            REAL,
    host            TEXT NOT NULL,
    archived        INTEGER NOT NULL DEFAULT 0,
    staged_path     TEXT,
    scope             TEXT,
    enriched_at       TEXT,
    enriched_modified TEXT,
    enrich_model      TEXT,
    prompt_version    INTEGER,
    enrich_status     TEXT,
    last_error        TEXT,
    attempts          INTEGER NOT NULL DEFAULT 0,
    redaction_count   INTEGER,
    tokens_in         INTEGER,
    tokens_out        INTEGER,
    tags_source       TEXT,
    updated_at        INTEGER NOT NULL DEFAULT 0
);
";

/// Build a genuine v5 DB on disk: the v5 schema, the `export_meta` counter, two rows carrying
/// NON-rowid-order revisions (10, 20), the counter seeded to 20, and the v5 triggers — then
/// `user_version = 5`. The rows are inserted BEFORE the triggers exist so their explicit revisions
/// stick (exactly how a post-v5-migration DB looks).
fn build_v5_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(V5_SESSIONS_SQL).unwrap();
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);
         CREATE TABLE IF NOT EXISTS export_meta (
             id       INTEGER PRIMARY KEY CHECK (id = 0),
             revision INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO export_meta (id, revision) VALUES (0, 0);",
    )
    .unwrap();
    // Rows with explicit, non-rowid-order revisions (id 1 -> rev 10, id 2 -> rev 20). If the v6
    // migration wrongly re-ran the v5 rowid backfill these would become 1 and 2.
    for (id, sid, rev) in [(1i64, UUID_A, 10i64), (2, UUID_B, 20)] {
        conn.execute(
            "INSERT INTO sessions (id, session_id, project_dir, transcript_path, modified, host, updated_at) \
             VALUES (?1, ?2, '/p', '/t', '2026-06-01T00:00:00Z', 'desk', ?3)",
            rusqlite::params![id, sid, rev],
        )
        .unwrap();
    }
    conn.execute("UPDATE export_meta SET revision = 20 WHERE id = 0", [])
        .unwrap();
    conn.execute_batch(V5_TRIGGERS_SQL).unwrap();
    conn.pragma_update(None, "user_version", 5i64).unwrap();
}

/// v5 -> v6 migration: adds the efficiency columns AND — the migration hazard this phase must audit —
/// PRESERVES every live `updated_at` revision and the counter (the v5 backfill is gated on
/// `from_version < 5`, so it does not re-run and rewind the cursor). BITES: remove the `from_version`
/// guard on the v5 backfill and reopening rewrites revisions to rowid order (1, 2) and reseeds the
/// counter to 2, failing every assertion below.
#[test]
fn v6_migration_from_v5_preserves_cursor_and_adds_efficiency_columns() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("v5.db");
    build_v5_db(&path);

    // Reopen: migrate v5 -> v6.
    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, SCHEMA_VERSION, "reopen migrates to v6");
    assert_eq!(SCHEMA_VERSION, 6, "this test pins the v5->v6 hop");

    // The live revisions are UNTOUCHED (not reset to rowid order), and the counter is preserved.
    assert_eq!(
        updated_at_of(&db, UUID_A),
        10,
        "row A's revision 10 is preserved across v5->v6"
    );
    assert_eq!(
        updated_at_of(&db, UUID_B),
        20,
        "row B's revision 20 is preserved across v5->v6"
    );
    assert_eq!(
        revision_counter(&db),
        20,
        "the export_meta counter is preserved (not reseeded)"
    );

    // The new efficiency columns exist and default to NULL (nothing computed yet).
    assert_eq!(efficiency_of(&db, UUID_A), (None, None, None, None));

    // The schema still functions: an efficiency write leaves the cursor put, then a content write
    // advances it to MAX+1 = 21 (strictly greater than every preserved revision).
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: r#"{"aggregate":{}}"#,
        cache_read_share: Some(0.7),
        tool_errors: 0,
        cost_usd: 0.0,
    }])
    .unwrap();
    assert_eq!(
        revision_counter(&db),
        20,
        "efficiency write does not move the preserved cursor"
    );
    assert!(
        db.record_enrich_skip(UUID_B, "work", crate::export::EnrichStatus::SkippedEmpty)
            .unwrap()
    );
    assert_eq!(
        revision_counter(&db),
        21,
        "the first content write after migration is MAX+1 = 21"
    );
    assert_eq!(updated_at_of(&db, UUID_B), 21);
}

/// The v6 migration is idempotent on reopen: the efficiency annotation survives, the cursor is
/// stable, and the columns still function. `migrate` is version-gated, so a re-open re-runs nothing.
#[test]
fn v6_migration_is_idempotent_on_reopen() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("s.db");
    {
        let db = Db::open_at(&path).unwrap();
        db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap(); // rev 1
        db.set_efficiency_many(&[EfficiencyWrite {
            session_id: UUID_A,
            efficiency_json: r#"{"aggregate":{"cache-read-share":0.42}}"#,
            cache_read_share: Some(0.42),
            tool_errors: 3,
            cost_usd: 1.25,
        }])
        .unwrap();
        assert_eq!(revision_counter(&db), 1, "efficiency write did not advance the cursor");
    }

    // Reopen: already v6, migrate short-circuits on the version gate; annotation + cursor stable.
    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, SCHEMA_VERSION);
    assert_eq!(revision_counter(&db), 1, "reopen must not re-run any backfill");
    let (json, share, errors, cost) = efficiency_of(&db, UUID_A);
    assert_eq!(json.as_deref(), Some(r#"{"aggregate":{"cache-read-share":0.42}}"#));
    assert_eq!((share, errors, cost), (Some(0.42), Some(3), Some(1.25)));

    // Re-open a third time: still stable, schema still works (a content write advances to 2).
    let db = Db::open_at(&path).unwrap();
    let before = revision_counter(&db);
    assert!(db.record_enrich_failure(UUID_A, "work", "boom").unwrap());
    assert_eq!(revision_counter(&db), before + 1);
}
