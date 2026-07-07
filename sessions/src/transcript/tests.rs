#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use session::ParsedSession;

use super::*;
use crate::db::Db;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn parsed(session_id: &str, project_dir: &Path, parent: &Path) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/marquee")),
        project_dir: project_dir.to_path_buf(),
        ai_title: Some("title".into()),
        first_prompt: Some("first".into()),
        command_name: None,
        git_branch: Some("main".into()),
        model: Some("claude-opus-4-8".into()),
        n_msgs: 4,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".into(),
        jsonl_paths: vec![parent.to_path_buf()],
    }
}

#[test]
fn resolves_live_layout_when_the_transcript_is_on_disk() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = tmp.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(&parent, "{}\n").unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, tmp.path(), &parent), "desk").unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();

    let (resolved_parent, resolved_subagents) = transcript_layout(&rec).expect("live transcript resolves");
    assert_eq!(resolved_parent, parent);
    assert_eq!(resolved_subagents, tmp.path().join(UUID_A).join("subagents"));
}

#[test]
fn falls_back_to_staged_when_the_live_transcript_is_gone() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = tmp.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(&parent, "{}\n").unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, tmp.path(), &parent), "desk").unwrap();

    // Stage a durable copy, then reap the live transcript (TTL) -- existence, not the `archived`
    // flag, must drive resolution.
    let staged_dir = tmp.path().join("staged").join(UUID_A);
    std::fs::create_dir_all(&staged_dir).unwrap();
    let staged_parent = staged_dir.join(format!("{UUID_A}.jsonl"));
    std::fs::write(&staged_parent, "{}\n").unwrap();
    db.set_staged_path(UUID_A, &staged_dir).unwrap();
    std::fs::remove_file(&parent).unwrap();

    let rec = db.get(UUID_A).unwrap().unwrap();
    let (resolved_parent, resolved_subagents) = transcript_layout(&rec).expect("staged copy resolves");
    assert_eq!(resolved_parent, staged_parent);
    assert_eq!(resolved_subagents, staged_dir.join("subagents"));
}

#[test]
fn returns_none_when_neither_live_nor_staged_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = tmp.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(&parent, "{}\n").unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, tmp.path(), &parent), "desk").unwrap();
    std::fs::remove_file(&parent).unwrap();

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert!(
        transcript_layout(&rec).is_none(),
        "no live transcript and no staged copy means nothing to parse"
    );
}
