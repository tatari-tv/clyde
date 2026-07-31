//! Schema v11 tests: `activity_at` / `parse_version`, the dormancy definition they feed, and the
//! narrow trigger-suppressed backfill that fills them.
//!
//! Their own submodule for the same reason `efficiency.rs` is one: `db/tests.rs` is near the
//! 1500-line limit and this is a self-contained surface.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use session::ParsedSession;

use crate::db::Db;
use crate::model::Filters;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

/// The dormancy cutoff every test here uses: 7 days ago, the configured default.
fn cutoff() -> DateTime<Utc> {
    Utc::now() - Duration::days(7)
}

/// A catalog row with an explicit `(activity_at, modified)` pair. `modified` is the filesystem mtime
/// and `activity_at` the MAX message timestamp, and separating them is the whole point: the defect
/// this schema version fixes is a host where the two disagree.
fn parsed(session_id: &str, activity_at: Option<DateTime<Utc>>, modified: DateTime<Utc>) -> ParsedSession {
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
        created: Some(Utc::now() - Duration::days(31)),
        activity_at,
        modified,
        body: "some body text".to_string(),
        jsonl_paths: vec![PathBuf::from("/tmp/a.jsonl")],
    }
}

/// THE defect (item B). A session whose messages are all 30 days old but whose files were ALL touched
/// `now` -- a Syncthing sync, a restore, a `cp -r` -- is still dormant, and BOTH filters say so.
///
/// This is what stood between a dormant session and permanent unpriceability: `stage_dormant` runs on
/// every `clyde session reindex` plus a 6h timer, and it drives off `staging_candidates`. With the
/// filter on mtime, a wholesale reset made every session look fresh, the sweep found no work, and the
/// transcripts it would have copied aged off disk.
///
/// BITES: revert either filter to `r.modified` and that filter returns EMPTY, because the mtime says
/// the session was touched a moment ago. Both call sites are asserted separately, because the
/// register's own count of them drifted once already.
#[test]
fn a_wholesale_mtime_reset_does_not_hide_a_dormant_session() {
    let db = Db::open_memory().unwrap();
    let thirty_days_ago = Utc::now() - Duration::days(30);
    db.upsert_session(&parsed(UUID_A, Some(thirty_days_ago), Utc::now()), "host-01")
        .unwrap();

    let staging: Vec<String> = db
        .staging_candidates(Some(cutoff()))
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(
        staging,
        vec![UUID_A.to_string()],
        "staging_candidates must still see the session as dormant despite the fresh mtime"
    );

    let enrich: Vec<String> = db
        .enrich_candidates(Some(cutoff()), 1, 3, false)
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(
        enrich,
        vec![UUID_A.to_string()],
        "enrich_candidates must apply the same definition of dormant"
    );

    // And the mtime really is fresh, so the assertions above are not passing for a boring reason.
    let row = db.get(UUID_A).unwrap().unwrap();
    assert!(row.modified > cutoff(), "the row's mtime is deliberately fresh");
    assert_eq!(row.dormancy_at(), thirty_days_ago, "dormancy reads the activity time");
}

/// The backfill WINDOW is behavior-neutral. A row whose `activity_at` is NULL -- every existing row,
/// between the v11 migration and the reindex that fills it -- is filtered exactly as it is today, via
/// `dormancy_at()`'s `modified` fallback. No session that is swept now stops being swept.
///
/// BITES: make `dormancy_at()` return something other than `modified` on `None` (say `MIN_UTC`) and
/// the fresh row below is wrongly reported dormant.
#[test]
fn a_null_activity_at_falls_back_to_mtime_exactly_as_today() {
    let db = Db::open_memory().unwrap();
    // Fresh mtime, no activity time: must NOT be dormant.
    db.upsert_session(&parsed(UUID_A, None, Utc::now()), "host-01").unwrap();
    // Old mtime, no activity time: must BE dormant.
    db.upsert_session(&parsed(UUID_B, None, Utc::now() - Duration::days(30)), "host-01")
        .unwrap();

    let staging: Vec<String> = db
        .staging_candidates(Some(cutoff()))
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(
        staging,
        vec![UUID_B.to_string()],
        "an un-backfilled row is filtered by mtime, i.e. today's behavior"
    );

    let fresh = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(fresh.activity_at, None);
    assert_eq!(fresh.dormancy_at(), fresh.modified, "the fallback IS `modified`");
}

/// Activity time is not a blank check in the other direction either: a session that last spoke an
/// hour ago is fresh even if its transcript's mtime is ancient (a `touch -t`, a restore from an
/// archive that preserved mtimes).
#[test]
fn a_recently_active_session_is_fresh_even_with_an_ancient_mtime() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(
            UUID_A,
            Some(Utc::now() - Duration::hours(1)),
            Utc::now() - Duration::days(30),
        ),
        "host-01",
    )
    .unwrap();

    assert!(
        db.staging_candidates(Some(cutoff())).unwrap().is_empty(),
        "a session that spoke an hour ago is not dormant, whatever its mtime says"
    );
}

/// The skip predicate is `(mtime, parse_version)`, and a stale `parse_version` reports
/// [`crate::db::Upsert::Backfilled`] rather than re-running the content UPDATE. That distinction is
/// the whole reason the backfill is cheap: the content arm NULLs `efficiency_json`, which would force
/// a full recompute that re-reads every transcript.
///
/// BITES: drop the `parse_version` half of the skip key and the second upsert reports
/// `SkippedUnchanged`, so the row's `activity_at` is never filled.
#[test]
fn a_stale_parse_version_reports_backfilled_not_skipped() {
    use crate::db::Upsert;

    let db = Db::open_memory().unwrap();
    let row = parsed(UUID_A, Some(Utc::now() - Duration::days(30)), Utc::now());
    assert_eq!(db.upsert_session(&row, "host-01").unwrap(), Upsert::Inserted);
    // A fresh INSERT writes `parse_version`, so the very next pass skips it. If the INSERT arm left
    // the column NULL, this row would be re-backfilled on every reindex, forever.
    assert_eq!(
        db.upsert_session(&row, "host-01").unwrap(),
        Upsert::SkippedUnchanged,
        "an INSERT must write parse_version, or the row is backfilled forever"
    );

    // Now simulate a pre-v11 row: same content, NULL parse_version.
    db.conn
        .execute("UPDATE sessions SET parse_version = NULL, activity_at = NULL", [])
        .unwrap();
    assert_eq!(
        db.upsert_session(&row, "host-01").unwrap(),
        Upsert::Backfilled,
        "unchanged content + stale parse_version is a backfill, not a skip and not a content update"
    );
}

/// The backfill write is NARROW: it fills two columns and touches neither the efficiency annotation
/// nor the export cursor, and a second run reports zero backfills.
///
/// Both assertions guard a specific, measured hazard. Routing the backfill through the content UPDATE
/// arm would NULL `efficiency_json` on every row, and the next efficiency pass would then re-read all
/// 2,111 transcripts on desk.lan. A bare UPDATE (no trigger sandwich) would fire
/// `sessions_updated_at_update` for every row, bumping `updated_at` and making every
/// `session export --cursor` consumer re-fetch the entire catalog.
///
/// BITES both ways: route the fill through the content arm and `efficiency_json` goes NULL; drop the
/// sandwich in `set_parse_derived_many` and `updated_at` advances.
#[test]
fn the_backfill_leaves_efficiency_and_the_export_cursor_untouched() {
    use crate::db::EfficiencyWrite;

    let db = Db::open_memory().unwrap();
    let row = parsed(UUID_A, Some(Utc::now() - Duration::days(30)), Utc::now());
    db.upsert_session(&row, "host-01").unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: r#"{"session-id":"a","aggregate":{}}"#,
        cache_read_share: Some(0.5),
        tool_errors: 2,
        cost_usd: 1.25,
        outcome_json: r#"{"commits":[]}"#,
    }])
    .unwrap();

    let before_eff = db.get_efficiency_json(UUID_A).unwrap();
    let before_rev: i64 = db
        .conn
        .query_row("SELECT updated_at FROM sessions WHERE session_id = ?1", [UUID_A], |r| {
            r.get(0)
        })
        .unwrap();

    // Make the row look pre-v11, then run the backfill write the reindex loop would run.
    db.conn
        .execute("UPDATE sessions SET parse_version = NULL, activity_at = NULL", [])
        .unwrap();
    let cursor_after_setup: i64 = db
        .conn
        .query_row("SELECT updated_at FROM sessions WHERE session_id = ?1", [UUID_A], |r| {
            r.get(0)
        })
        .unwrap();
    let activity = Utc::now() - Duration::days(30);
    assert_eq!(
        db.set_parse_derived_many(&[crate::db::ParseDerivedWrite {
            session_id: UUID_A.to_string(),
            activity_at: Some(activity),
            title: row.title(),
        }])
        .unwrap(),
        1
    );

    assert_eq!(
        db.get_efficiency_json(UUID_A).unwrap(),
        before_eff,
        "the backfill must NOT invalidate the efficiency annotation"
    );
    let after_rev: i64 = db
        .conn
        .query_row("SELECT updated_at FROM sessions WHERE session_id = ?1", [UUID_A], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        after_rev, cursor_after_setup,
        "the backfill must NOT advance the export revision cursor"
    );
    assert!(before_rev > 0, "the row had a real revision to preserve");

    // The value landed, and the row is now skipped: a second pass finds nothing to backfill.
    assert_eq!(db.get(UUID_A).unwrap().unwrap().activity_at, Some(activity));
    assert_eq!(
        db.upsert_session(&row, "host-01").unwrap(),
        crate::db::Upsert::SkippedUnchanged,
        "a backfilled row is skipped on the next pass: the backfill drains itself"
    );
}

/// A transcript with NO parseable `timestamp` on any record yields `activity_at = None`
/// LEGITIMATELY, and it must still terminate. `parse_version` is what makes that possible: it is
/// written even when the value is NULL, so the row is backfilled once and skipped thereafter.
///
/// BITES: gate the backfill on `activity_at IS NOT NULL` instead of `parse_version` and this row is
/// rewritten on every single reindex, forever.
#[test]
fn a_transcript_with_no_timestamps_is_backfilled_once_then_skipped() {
    use crate::db::Upsert;

    let db = Db::open_memory().unwrap();
    // `activity_at: None` with a fixed mtime: the parser found no timestamp anywhere.
    let row = parsed(UUID_A, None, Utc::now() - Duration::days(30));
    db.upsert_session(&row, "host-01").unwrap();
    db.conn.execute("UPDATE sessions SET parse_version = NULL", []).unwrap();

    assert_eq!(db.upsert_session(&row, "host-01").unwrap(), Upsert::Backfilled);
    // The batch write records `parse_version` even though the value is NULL. That is the termination.
    assert_eq!(
        db.set_parse_derived_many(&[crate::db::ParseDerivedWrite {
            session_id: UUID_A.to_string(),
            activity_at: None,
            title: row.title(),
        }])
        .unwrap(),
        1
    );
    assert_eq!(
        db.get(UUID_A).unwrap().unwrap().activity_at,
        None,
        "a transcript with no timestamps has no activity time, and that is a legitimate answer"
    );
    assert_eq!(
        db.upsert_session(&row, "host-01").unwrap(),
        Upsert::SkippedUnchanged,
        "and it is never reconsidered"
    );
}

/// Appending `activity_at` to `COLS` shifted `Db::catalog`'s five trailing columns, and that path is
/// the SILENT one: `efficiency_json` and `outcome_json` are both `Option<String>`, so an off-by-one
/// still type-checks and simply reads the neighbouring column.
///
/// The row therefore carries BOTH blobs populated, which is what makes this test able to fail at all:
/// with `outcome_json` NULL, a shift would leave `cache_read_share` reading NULL and the whole thing
/// would pass while mis-mapped.
///
/// BITES: subtract one from any `COLS_LEN + n` index in `map_catalog_entry` and the blob assertions
/// fail (the sibling `search_table` site fails LOUDLY instead, in the existing ranking tests, because
/// `row.get::<f64>` on a TEXT column errors).
#[test]
fn the_catalog_round_trips_both_blobs_after_cols_grew() {
    use crate::db::EfficiencyWrite;

    let db = Db::open_memory().unwrap();
    let activity = Utc::now() - Duration::days(30);
    db.upsert_session(&parsed(UUID_A, Some(activity), Utc::now()), "host-01")
        .unwrap();
    let eff = r#"{"session-id":"a","aggregate":{"raw":{"cost-usd":1.25}}}"#;
    let outcome = r#"{"commits":["abc123"],"files-edited":3}"#;
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: UUID_A,
        efficiency_json: eff,
        cache_read_share: Some(0.5),
        tool_errors: 2,
        cost_usd: 1.25,
        outcome_json: outcome,
    }])
    .unwrap();

    let entries = db.catalog(&Filters::default()).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(
        entry.efficiency_json.as_deref(),
        Some(eff),
        "efficiency_json mis-mapped"
    );
    assert_eq!(entry.outcome_json.as_deref(), Some(outcome), "outcome_json mis-mapped");
    assert_eq!(entry.cache_read_share, Some(0.5));
    assert_eq!(entry.tool_errors, Some(2));
    assert_eq!(entry.cost_usd, Some(1.25));
    // And the record prefix itself still maps, including the newly-appended column.
    assert_eq!(entry.record.session_id, UUID_A);
    assert_eq!(entry.record.activity_at, Some(activity));
    assert_eq!(entry.record.repo.as_deref(), None, "the v10 columns still map");
}

/// The migration: a real on-disk DB reaches at least v11, gains both columns, and gets its
/// `.pre-v11.bak` snapshot exactly once.
///
/// `>= 11` on purpose, not `== 11`: the next phase takes the schema to 12, and an equality assertion
/// here would fail on a correct implementation of it.
#[test]
fn opening_an_on_disk_db_migrates_to_v11_and_snapshots_once() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("sessions.db");

    // First open creates the schema at `user_version = 0` -> current; nothing pre-existing to protect.
    {
        let db = Db::open_at(&path).unwrap();
        db.upsert_session(&parsed(UUID_A, None, Utc::now()), "host-01").unwrap();
    }
    let snapshot = PathBuf::from(format!("{}.pre-v11.bak", path.display()));
    assert!(
        !snapshot.exists(),
        "a brand-new catalog has no pre-v11 state worth snapshotting"
    );

    // Now rewind the version to 10 to simulate a genuine pre-v11 catalog, and reopen.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 10i64).unwrap();
    }
    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert!(uv >= 11, "reopen migrates to at least v11, got {uv}");
    assert!(snapshot.exists(), "a genuine pre-v11 catalog is snapshotted first");

    // Both columns exist and are readable.
    let cols: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(sessions)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(cols.contains(&"activity_at".to_string()), "columns: {cols:?}");
    assert!(cols.contains(&"parse_version".to_string()), "columns: {cols:?}");

    // Snapshot-once: reopening an already-migrated DB does not rewrite it.
    let stamp = std::fs::metadata(&snapshot).unwrap().modified().unwrap();
    drop(db);
    Db::open_at(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&snapshot).unwrap().modified().unwrap(),
        stamp,
        "the snapshot predicate can never match again once the version is bumped"
    );
}

/// `title` is re-derived by the same narrow backfill, and the FTS index moves WITH the column.
///
/// The stored title for a session Claude never titled was the raw `first_prompt`, capped at 2,000 chars
/// (61 such rows on desk.lan). `PARSE_VERSION` 2 re-offers those rows so the new derivation lands
/// without a migration and without re-reading any transcript.
///
/// `sessions_fts` is a standalone FTS5 table maintained by explicit writes, NOT by triggers, so the
/// backfill has to update it too. Skipping that leaves search matching the old 2,000-char title while
/// every display surface shows the new one -- a silent desync, since nothing fails.
///
/// BITES: drop the `sessions_fts` UPDATE and the FTS assertion fails while the column assertion passes,
/// which is exactly the shape of the bug it guards.
#[test]
fn the_backfill_re_derives_the_title_and_keeps_fts_in_step() {
    let db = Db::open_memory().unwrap();
    let mut row = parsed(UUID_A, Some(Utc::now() - Duration::days(30)), Utc::now());
    // A session Claude never titled, whose first prompt is a multi-line agent launch: the reported shape.
    row.ai_title = None;
    row.first_prompt = Some(format!(
        "Implement exactly **Phase 2** of the design doc at:\n{}",
        "x".repeat(1_900)
    ));
    db.upsert_session(&row, "host-01").unwrap();

    // Plant the pre-v2 stored state: the raw untruncated prompt, in the column AND in the FTS row.
    let raw = row.first_prompt.clone().unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET parse_version = NULL, title = ?1 WHERE session_id = ?2",
            rusqlite::params![raw, UUID_A],
        )
        .unwrap();
    let rowid: i64 = db
        .conn
        .query_row("SELECT id FROM sessions WHERE session_id = ?1", [UUID_A], |r| r.get(0))
        .unwrap();
    db.conn
        .execute(
            "UPDATE sessions_fts SET title = ?1 WHERE rowid = ?2",
            rusqlite::params![raw, rowid],
        )
        .unwrap();

    assert_eq!(
        db.upsert_session(&row, "host-01").unwrap(),
        crate::db::Upsert::Backfilled
    );
    let want = row.title().unwrap();
    assert_eq!(
        db.set_parse_derived_many(&[crate::db::ParseDerivedWrite {
            session_id: UUID_A.to_string(),
            activity_at: row.activity_at,
            title: Some(want.clone()),
        }])
        .unwrap(),
        1
    );

    // The column carries the shaped title, not the 1,900-char blob.
    let stored: String = db
        .conn
        .query_row("SELECT title FROM sessions WHERE session_id = ?1", [UUID_A], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, want);
    assert_eq!(stored, "Implement exactly **Phase 2** of the design doc at:");
    assert!(stored.chars().count() < 200, "the 2,000-char title is gone: {stored:?}");

    // And so does the FTS row, so search and display agree.
    let indexed: String = db
        .conn
        .query_row("SELECT title FROM sessions_fts WHERE rowid = ?1", [rowid], |r| r.get(0))
        .unwrap();
    assert_eq!(indexed, want, "the FTS title must be re-indexed with the column");
}
