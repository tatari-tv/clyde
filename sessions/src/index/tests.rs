#![allow(clippy::unwrap_used)]

use super::*;
use crate::Db;
use common::repo::PathMap;
use std::fs;
use std::path::Path;
use std::process::Command;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_LIVE: &str = "1a2b3c4d-0000-4000-8000-000000000001";
const UUID_VANISHED_NEW: &str = "1a2b3c4d-0000-4000-8000-000000000002";

fn write(path: &Path, lines: &[&str]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, lines.join("\n")).unwrap();
}

fn git_init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let s = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
}

fn add_origin(dir: &Path, url: &str) {
    let s = Command::new("git")
        .args(["remote", "add", "origin", url])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
}

fn user_message(cwd: &Path, session_id: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"user","cwd":"{}","timestamp":"{timestamp}","sessionId":"{session_id}","message":{{"content":"hello"}}}}"#,
        cwd.display()
    )
}

#[test]
fn reindex_ingests_then_skips_unchanged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let proj = projects.join("-home-saidler-repos-tatari-tv-marquee");
    write(
        &proj.join(format!("{UUID_A}.jsonl")),
        &[
            r#"{"type":"user","cwd":"/home/saidler/repos/tatari-tv/marquee","gitBranch":"main","timestamp":"2026-06-21T10:00:00Z","message":{"content":"set up the terraform marquee bucket"}}"#,
            r#"{"type":"ai-title","aiTitle":"Terraform Marquee bucket","sessionId":"x"}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:05Z","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"creating the S3 bucket in us-east-1"}]}}"#,
        ],
    );

    let db = Db::open_memory().unwrap();
    let repo_root = tmp.path().join("repos");
    let stats = reindex(&db, &projects, &repo_root).unwrap();
    assert_eq!(stats.scanned, 1);
    assert_eq!(stats.upserted, 1);
    assert_eq!(stats.skipped_unchanged, 0);
    assert_eq!(db.count().unwrap(), 1);

    // Search reaches the indexed record by title and by body-only term.
    assert_eq!(
        db.search("terraform", None, false, crate::SortBy::Relevance)
            .unwrap()
            .count,
        1
    );
    assert_eq!(
        db.search("us-east-1", None, false, crate::SortBy::Relevance)
            .unwrap()
            .count,
        1
    );

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.title.as_deref(), Some("Terraform Marquee bucket"));
    assert_eq!(rec.git_branch.as_deref(), Some("main"));

    // A second reindex with no file changes skips everything.
    let stats2 = reindex(&db, &projects, &repo_root).unwrap();
    assert_eq!(stats2.scanned, 1);
    assert_eq!(stats2.upserted, 0);
    assert_eq!(stats2.skipped_unchanged, 1);
}

#[test]
fn reindex_preserves_tags_across_runs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let path = projects.join("proj").join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"hello"}}"#],
    );

    let db = Db::open_memory().unwrap();
    let repo_root = tmp.path().join("repos");
    reindex(&db, &projects, &repo_root).unwrap();
    db.set_tags(UUID_A, &["keepme".into()]).unwrap();

    // Rewrite with new content; whether the second reindex re-upserts (mtime advanced) or
    // skips (coarse mtime resolution), the user tag must survive either path.
    write(
        &path,
        &[r#"{"type":"user","timestamp":"2026-06-21T11:00:00Z","message":{"content":"hello again"}}"#],
    );
    reindex(&db, &projects, &repo_root).unwrap();

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["keepme".to_string()], "tags survive reindex");
}

#[test]
fn reindex_empty_projects_is_ok() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();
    let stats = reindex(&db, &tmp.path().join("nonexistent"), &tmp.path().join("repos")).unwrap();
    assert_eq!(stats, crate::ReindexStats::default());
}

/// AC: a reindex populates `repo`, `repo_source`, `repo_rank`, and one `repo_paths` row for a
/// session whose cwd exists.
#[test]
fn reindex_persists_repo_attribution_for_a_live_worktree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo_dir = tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("repos")
        .join("tatari-tv")
        .join("clyde");
    git_init(&repo_dir);
    add_origin(&repo_dir, "git@github.com:tatari-tv/clyde.git");

    let projects = tmp.path().join("projects");
    write(
        &projects.join("proj").join(format!("{UUID_LIVE}.jsonl")),
        &[&user_message(&repo_dir, UUID_LIVE, "2026-06-21T10:00:00Z")],
    );

    let db = Db::open_memory().unwrap();
    let repo_root = tmp.path().join("repos");
    reindex(&db, &projects, &repo_root).unwrap();

    let row = db.repo_of(UUID_LIVE).unwrap().unwrap();
    assert_eq!(row.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row.source.as_deref(), Some("git-origin"));
    assert_eq!(row.rank, 0);
    assert_eq!(
        db.repo_for_path(&repo_dir),
        Some("tatari-tv/clyde".to_string()),
        "every rule-1 success records the cwd into repo_paths"
    );
}

/// The decay-regression test, the doc's whole point: a session resolved once at rank 0 keeps its
/// answer after the directory that produced it is gone, AND a brand-new session at that same
/// vanished path still resolves via `KnownPath` (rule 2), because rule 1 recorded the mapping into
/// `repo_paths` while the directory was alive.
#[test]
fn reindex_decay_regression_keeps_prior_repo_and_resolves_new_session_via_known_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo_dir = tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("repos")
        .join("tatari-tv")
        .join("clyde")
        .join("main");
    git_init(&repo_dir);
    add_origin(&repo_dir, "git@github.com:tatari-tv/clyde.git");

    let projects = tmp.path().join("projects");
    write(
        &projects.join("proj").join(format!("{UUID_A}.jsonl")),
        &[&user_message(&repo_dir, UUID_A, "2026-06-21T10:00:00Z")],
    );

    let db = Db::open_memory().unwrap();
    let repo_root = tmp.path().join("repos");
    reindex(&db, &projects, &repo_root).unwrap();

    let row = db.repo_of(UUID_A).unwrap().unwrap();
    assert_eq!(row.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row.source.as_deref(), Some("git-origin"));
    assert_eq!(row.rank, 0);

    // The worktree is cleaned up -- exactly the `clyde-ft`/`clyde-report` sibling-worktree churn
    // the design doc measures. A brand-new session lands at the SAME now-vanished path.
    fs::remove_dir_all(&repo_dir).unwrap();
    assert!(!repo_dir.exists());
    write(
        &projects.join("proj").join(format!("{UUID_VANISHED_NEW}.jsonl")),
        &[&user_message(&repo_dir, UUID_VANISHED_NEW, "2026-06-25T10:00:00Z")],
    );

    reindex(&db, &projects, &repo_root).unwrap();

    // UUID_A's transcript did not change, but repo resolution ran again anyway (it is not gated
    // behind the content-mtime skip) -- rule 1 now fails (dir gone), so rule 2 would answer
    // `KnownPath` at rank 1. The strictly-improving write REJECTS that: the stored rank-0 answer
    // must survive untouched.
    let after = db.repo_of(UUID_A).unwrap().unwrap();
    assert_eq!(
        after.repo.as_deref(),
        Some("tatari-tv/clyde"),
        "the decay regression: repo must not change"
    );
    assert_eq!(
        after.source.as_deref(),
        Some("git-origin"),
        "provenance must not regress either"
    );
    assert_eq!(after.rank, 0);

    // The NEW session at the same vanished path resolves via the learned map (rule 2), not rule 4's
    // guess -- `repo_paths` was populated while the directory was alive, on the FIRST reindex.
    let new = db.repo_of(UUID_VANISHED_NEW).unwrap().unwrap();
    assert_eq!(new.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(new.source.as_deref(), Some("known-path"));
    assert_eq!(new.rank, 1);
}
