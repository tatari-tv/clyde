#![allow(clippy::unwrap_used)]

use super::*;
use std::fs;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn set_mtime(path: &Path, when: SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(when).unwrap();
}

#[test]
fn stages_parent_and_subagents_then_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("projects").join("-home-saidler-repos-foo");
    let staged_root = tmp.path().join("staged");

    write(&project.join(format!("{UUID_A}.jsonl")), "parent content\n");
    write(
        &project.join(UUID_A).join("subagents").join("agent-1.jsonl"),
        "sub one\n",
    );
    write(
        &project.join(UUID_A).join("subagents").join("agent-2.jsonl"),
        "sub two\n",
    );

    let staged = stage_session(&project, UUID_A, &staged_root).unwrap();
    assert_eq!(staged.files_total, 3);
    assert_eq!(staged.files_copied, 3);
    assert_eq!(staged.dir, staged_root.join(UUID_A));

    // Content is preserved at the mirrored layout.
    assert_eq!(
        fs::read_to_string(staged_root.join(UUID_A).join(format!("{UUID_A}.jsonl"))).unwrap(),
        "parent content\n"
    );
    assert_eq!(
        fs::read_to_string(staged_root.join(UUID_A).join("subagents").join("agent-1.jsonl")).unwrap(),
        "sub one\n"
    );

    // Re-staging with no source change copies nothing (staged copy is >= source mtime).
    let again = stage_session(&project, UUID_A, &staged_root).unwrap();
    assert_eq!(again.files_total, 3);
    assert_eq!(again.files_copied, 0);
}

#[test]
fn restages_when_source_is_newer() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("proj");
    let staged_root = tmp.path().join("staged");
    let parent = project.join(format!("{UUID_A}.jsonl"));
    write(&parent, "v1\n");
    stage_session(&project, UUID_A, &staged_root).unwrap();

    let staged_file = staged_root.join(UUID_A).join(format!("{UUID_A}.jsonl"));
    // Rewrite the source and force its mtime strictly newer than the staged copy.
    fs::write(&parent, "v2\n").unwrap();
    let now = SystemTime::now();
    set_mtime(&staged_file, now - Duration::from_secs(10));
    set_mtime(&parent, now);

    let again = stage_session(&project, UUID_A, &staged_root).unwrap();
    assert_eq!(again.files_copied, 1, "newer source is re-copied");
    assert_eq!(fs::read_to_string(&staged_file).unwrap(), "v2\n");
}

#[test]
fn missing_parent_with_no_subagents_stages_nothing() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    let staged = stage_session(&project, UUID_A, &tmp.path().join("staged")).unwrap();
    assert_eq!(staged.files_total, 0);
    assert_eq!(staged.files_copied, 0);
}
