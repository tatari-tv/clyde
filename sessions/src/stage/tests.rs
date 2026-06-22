#![allow(clippy::unwrap_used)]

use super::*;
use crate::{Db, reindex};
use chrono::Duration;
use std::fs;
use std::path::Path;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn write(path: &Path, line: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, line).unwrap();
}

fn session_line(ts: &str) -> String {
    format!(r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"hello"}}}}"#)
}

#[test]
fn stage_dormant_filters_by_cutoff_and_records_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");

    let proj_a = projects.join("-home-saidler-repos-a");
    let proj_b = projects.join("-home-saidler-repos-b");
    write(
        &proj_a.join(format!("{UUID_A}.jsonl")),
        &session_line("2026-06-21T10:00:00Z"),
    );
    write(
        &proj_b.join(format!("{UUID_B}.jsonl")),
        &session_line("2026-06-21T10:00:00Z"),
    );

    let db = Db::open_memory().unwrap();
    reindex(&db, &projects).unwrap();

    // Make session A look dormant (old mtime) and B fresh, by setting file mtimes then reindexing.
    set_old_mtime(&proj_a.join(format!("{UUID_A}.jsonl")), Duration::days(30));
    reindex(&db, &projects).unwrap();

    // Cutoff = 7 days ago: only A (30d old) is dormant.
    let cutoff = Utc::now() - Duration::days(7);
    let stats = stage_dormant(&db, Some(cutoff), &staged_root).unwrap();
    assert_eq!(stats.considered, 1, "only the dormant session considered");
    assert_eq!(stats.staged, 1);
    assert_eq!(stats.files_copied, 1);

    // A's staged_path is recorded; the file exists; B was untouched.
    let a = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(a.staged_path.as_deref(), Some(staged_root.join(UUID_A).as_path()));
    assert!(staged_root.join(UUID_A).join(format!("{UUID_A}.jsonl")).exists());
    assert!(db.get(UUID_B).unwrap().unwrap().staged_path.is_none());
}

#[test]
fn stage_dormant_all_stages_everything() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    write(
        &projects.join("a").join(format!("{UUID_A}.jsonl")),
        &session_line("2026-06-21T10:00:00Z"),
    );
    write(
        &projects.join("b").join(format!("{UUID_B}.jsonl")),
        &session_line("2026-06-21T10:00:00Z"),
    );

    let db = Db::open_memory().unwrap();
    reindex(&db, &projects).unwrap();

    let stats = stage_dormant(&db, None, &staged_root).unwrap();
    assert_eq!(stats.considered, 2);
    assert_eq!(stats.staged, 2);

    // A second sweep is a no-op (already current).
    let again = stage_dormant(&db, None, &staged_root).unwrap();
    assert_eq!(again.considered, 2);
    assert_eq!(again.staged, 0);
    assert_eq!(again.up_to_date, 2);
}

#[test]
fn staged_copy_survives_ttl_reap() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    let transcript = projects.join("a").join(format!("{UUID_A}.jsonl"));
    write(&transcript, &session_line("2026-06-21T10:00:00Z"));

    let db = Db::open_memory().unwrap();
    reindex(&db, &projects).unwrap();
    stage_dormant(&db, None, &staged_root).unwrap();

    // Simulate Claude's 30-day TTL reaping the live transcript, then reconcile.
    fs::remove_file(&transcript).unwrap();
    db.reconcile_archived().unwrap();

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert!(rec.archived, "reaped transcript flags archived");
    // The staged copy is still on disk and still recorded — open/trace can resolve it.
    let staged = rec.staged_path.unwrap();
    assert!(staged.join(format!("{UUID_A}.jsonl")).exists());
}

fn set_old_mtime(path: &Path, ago: Duration) {
    let when = std::time::SystemTime::now() - ago.to_std().unwrap();
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(when).unwrap();
}
