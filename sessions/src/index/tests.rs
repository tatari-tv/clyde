#![allow(clippy::unwrap_used)]

use super::*;
use crate::Db;
use std::fs;
use std::path::Path;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn write(path: &Path, lines: &[&str]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, lines.join("\n")).unwrap();
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
    let stats = reindex(&db, &projects).unwrap();
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
    let stats2 = reindex(&db, &projects).unwrap();
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
    reindex(&db, &projects).unwrap();
    db.set_tags(UUID_A, &["keepme".into()]).unwrap();

    // Rewrite with new content; whether the second reindex re-upserts (mtime advanced) or
    // skips (coarse mtime resolution), the user tag must survive either path.
    write(
        &path,
        &[r#"{"type":"user","timestamp":"2026-06-21T11:00:00Z","message":{"content":"hello again"}}"#],
    );
    reindex(&db, &projects).unwrap();

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["keepme".to_string()], "tags survive reindex");
}

#[test]
fn reindex_empty_projects_is_ok() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();
    let stats = reindex(&db, &tmp.path().join("nonexistent")).unwrap();
    assert_eq!(stats, crate::ReindexStats::default());
}
