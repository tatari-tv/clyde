#![allow(clippy::unwrap_used)]

//! Phase 2 (`files-touched` catalog column) tests: the v6 migration, live-upsert population, the
//! narrow `set_files_touched` writer, the backfill-candidate predicate, and the `reindex --reparse`
//! staged pass. Split out of the parent `db/tests.rs` to keep that file under the line-count limit.

use std::path::Path;

use rusqlite::{Connection, params};

use super::{UUID_A, UUID_B, UUID_C, parsed, updated_at_of};
use crate::Db;
use crate::db::{apply_pragmas, migrate};

/// How many columns named `files_touched` the `sessions` table has (must always be exactly 1 after
/// the v6 migration, whether run once or re-run over an existing catalog).
fn files_touched_column_count(conn: &Connection) -> usize {
    let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(rusqlite::Result::ok)
        .filter(|name| name == "files_touched")
        .count()
}

/// Every column of a session row EXCEPT those in `exclude`, read as a canonical `(name, string)`
/// pair so two snapshots compare byte-for-byte. Used to prove the narrow `set_files_touched` writer
/// disturbs nothing but its one column (and the trigger-owned `updated_at`).
fn row_columns_except(db: &Db, session_id: &str, exclude: &[&str]) -> Vec<(String, String)> {
    let mut stmt = db.conn.prepare("SELECT * FROM sessions WHERE session_id = ?1").unwrap();
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    stmt.query_row(params![session_id], |row| {
        let mut out = Vec::new();
        for (i, name) in names.iter().enumerate() {
            if exclude.contains(&name.as_str()) {
                continue;
            }
            let value: rusqlite::types::Value = row.get(i)?;
            out.push((name.clone(), format!("{value:?}")));
        }
        Ok(out)
    })
    .unwrap()
}

/// Write `lines` as a newline-joined JSONL transcript, creating parent dirs.
fn write_jsonl(path: &Path, lines: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, lines.join("\n")).unwrap();
}

/// The path the `Read` tool_use below touches, so the staged-pass tests assert an exact set.
const STAGED_TOUCHED_PATH: &str = "/home/saidler/repos/tatari-tv/clyde/src/bar.rs";

/// A one-turn transcript whose assistant message is a `Read` tool_use, so parsing yields
/// `files_touched = ["…/bar.rs"]`.
const STAGED_TRANSCRIPT: &[&str] = &[
    r#"{"type":"user","cwd":"/home/saidler/repos/tatari-tv/clyde","gitBranch":"main","timestamp":"2026-06-21T10:00:00Z","message":{"content":"read the file"}}"#,
    r#"{"type":"assistant","timestamp":"2026-06-21T10:00:05Z","message":{"model":"claude-opus-4-8","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/home/saidler/repos/tatari-tv/clyde/src/bar.rs"}}]}}"#,
];

#[test]
fn migration_v6_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    apply_pragmas(&conn).unwrap();
    migrate(&conn).unwrap();
    assert_eq!(
        files_touched_column_count(&conn),
        1,
        "column present after first migrate"
    );
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, 6);

    // Simulate re-running the migration over an already-catalogued v5 DB: force the version back and
    // migrate again. The v6 `ensure_column` must be a no-op (idempotent ALTER guard), not a
    // "duplicate column name" error, and the column count must stay exactly 1.
    conn.pragma_update(None, "user_version", 5i64).unwrap();
    migrate(&conn).unwrap();
    assert_eq!(
        files_touched_column_count(&conn),
        1,
        "no duplicate files_touched column"
    );
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, 6);
}

#[test]
fn upsert_populates_files_touched_sorted_json() {
    let db = Db::open_memory().unwrap();
    let mut p = parsed(UUID_A, "/tmp/a.jsonl");
    // Insert out of order + a duplicate to prove BTreeSet dedup + sort drives serialization.
    p.files_touched = ["/z/last.rs".into(), "/a/first.rs".into(), "/a/first.rs".into()]
        .into_iter()
        .collect();
    db.upsert_session(&p, "desk").unwrap();
    assert_eq!(
        db.files_touched_of(UUID_A).unwrap().as_deref(),
        Some(r#"["/a/first.rs","/z/last.rs"]"#)
    );
}

#[test]
fn upsert_empty_files_touched_is_empty_array_not_null() {
    let db = Db::open_memory().unwrap();
    let p = parsed(UUID_A, "/tmp/a.jsonl"); // default files_touched is empty
    db.upsert_session(&p, "desk").unwrap();
    // Parsed-but-no-file-tools is `[]`, never NULL: NULL is reserved for "not yet parsed / unknowable".
    assert_eq!(db.files_touched_of(UUID_A).unwrap().as_deref(), Some("[]"));
}

#[test]
fn set_files_touched_leaves_every_other_column_byte_identical() {
    let db = Db::open_memory().unwrap();
    let p = parsed(UUID_A, "/tmp/a.jsonl");
    db.upsert_session(&p, "desk").unwrap();
    // Give the row realistic non-parse state so the snapshot covers enrichment + tags columns too.
    db.set_tags(UUID_A, &["keepme".into()]).unwrap();

    // Exclude only the column under test and the trigger-owned opaque cursor.
    let before = row_columns_except(&db, UUID_A, &["files_touched", "updated_at"]);
    let rev_before = updated_at_of(&db, UUID_A);

    assert!(db.set_files_touched(UUID_A, r#"["/x.rs"]"#).unwrap());

    let after = row_columns_except(&db, UUID_A, &["files_touched", "updated_at"]);
    assert_eq!(before, after, "narrow writer touched a column other than files_touched");
    assert_eq!(
        db.files_touched_of(UUID_A).unwrap().as_deref(),
        Some(r#"["/x.rs"]"#),
        "the one column it targets IS updated"
    );
    assert!(
        updated_at_of(&db, UUID_A) > rev_before,
        "the v5 trigger advanced the cursor (set_files_touched does not set updated_at itself)"
    );
}

#[test]
fn set_files_touched_absent_session_returns_false() {
    let db = Db::open_memory().unwrap();
    assert!(!db.set_files_touched("no-such-session", "[]").unwrap());
}

#[test]
fn backfill_candidates_require_null_column_and_staged_path() {
    let db = Db::open_memory().unwrap();
    // A: populated (upsert wrote a set) + staged -> NOT a candidate.
    let mut a = parsed(UUID_A, "/tmp/a.jsonl");
    a.files_touched = ["/a.rs".into()].into_iter().collect();
    db.upsert_session(&a, "desk").unwrap();
    db.set_staged_path(UUID_A, Path::new("/tmp/staged/a")).unwrap();
    // B: NULL column + staged -> IS a candidate.
    db.upsert_session(&parsed(UUID_B, "/tmp/b.jsonl"), "desk").unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET files_touched = NULL WHERE session_id = ?1",
            params![UUID_B],
        )
        .unwrap();
    db.set_staged_path(UUID_B, Path::new("/tmp/staged/b")).unwrap();
    // C: NULL column but NO staged path -> NOT a candidate (unreachable, stays NULL).
    db.upsert_session(&parsed(UUID_C, "/tmp/c.jsonl"), "desk").unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET files_touched = NULL WHERE session_id = ?1",
            params![UUID_C],
        )
        .unwrap();

    let ids: Vec<String> = db
        .files_touched_backfill_candidates()
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(ids, vec![UUID_B.to_string()], "only NULL-column + staged rows qualify");
}

/// Phase 2 criterion: the staged pass fills a NULL row from its staged copy via the narrow writer,
/// leaving the contract-critical parse columns untouched. The isolated byte-identity guard for the
/// narrow writer itself is [`set_files_touched_leaves_every_other_column_byte_identical`]; this test
/// proves the reparse wiring reaches the staged pass and preserves `modified`/`transcript_path`.
#[test]
fn reparse_staged_pass_populates_archived_row_via_narrow_writer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let live = projects.join("proj").join(format!("{UUID_A}.jsonl"));
    write_jsonl(&live, STAGED_TRANSCRIPT);

    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    crate::reindex(&db, &projects).unwrap();

    // Stage a durable copy mirroring the live layout, and record its dir on the row.
    let staged = tmp.path().join("staged").join(UUID_A);
    write_jsonl(&staged.join(format!("{UUID_A}.jsonl")), STAGED_TRANSCRIPT);
    db.set_staged_path(UUID_A, &staged).unwrap();

    // Reap the live transcript and simulate a pre-v6 row (files_touched NULL) so the staged pass owns
    // the fill: the live pass can no longer reach it (the scan finds nothing).
    std::fs::remove_file(&live).unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET files_touched = NULL WHERE session_id = ?1",
            params![UUID_A],
        )
        .unwrap();

    let before = db.get(UUID_A).unwrap().unwrap();
    let rs = crate::reparse(&db, &projects).unwrap();

    assert_eq!(rs.live_scanned, 0, "live transcript is gone, nothing to scan");
    assert_eq!(rs.staged_candidates, 1);
    assert_eq!(rs.staged_populated, 1);
    assert_eq!(rs.failed, 0);
    assert_eq!(
        db.files_touched_of(UUID_A).unwrap().as_deref(),
        Some(format!(r#"["{STAGED_TOUCHED_PATH}"]"#).as_str()),
    );

    // The narrow writer must not disturb the contract fields the v5 cursor + dormancy depend on.
    let after = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(before.modified, after.modified, "modified unchanged");
    assert_eq!(
        before.transcript_path, after.transcript_path,
        "transcript_path unchanged"
    );
    assert_eq!(before.title, after.title, "title unchanged");
    assert_eq!(before.n_msgs, after.n_msgs, "n_msgs unchanged");
    assert_eq!(before.created, after.created, "created unchanged");
    assert_eq!(before.cwd, after.cwd, "cwd unchanged");
}

/// Phase 2 criterion: a row with neither a reachable live nor a staged transcript stays NULL,
/// without erroring the run (NULL = "unknowable"). Such a row is not even a staged candidate.
#[test]
fn reparse_leaves_unreachable_row_null() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let live = projects.join("proj").join(format!("{UUID_A}.jsonl"));
    write_jsonl(&live, STAGED_TRANSCRIPT);

    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    crate::reindex(&db, &projects).unwrap();

    // Reap the live transcript, no staged copy, and seed NULL (pre-v6). Nothing can parse it.
    std::fs::remove_file(&live).unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET files_touched = NULL WHERE session_id = ?1",
            params![UUID_A],
        )
        .unwrap();

    let rs = crate::reparse(&db, &projects).unwrap();
    assert_eq!(rs.failed, 0, "an unreachable row is not a failure");
    assert_eq!(rs.staged_candidates, 0, "no staged_path -> not a candidate");
    assert!(
        db.files_touched_of(UUID_A).unwrap().is_none(),
        "unreachable row stays NULL (unknowable)"
    );
}
