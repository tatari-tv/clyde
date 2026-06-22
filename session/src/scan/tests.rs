#![allow(clippy::unwrap_used)]

use super::*;
use std::fs;
use tempfile::TempDir;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn touch(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn missing_projects_dir_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("nope");
    assert!(find_session_files(&missing).unwrap().is_empty());
}

#[test]
fn finds_parents_and_subagents_skips_noise() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path();
    let proj = projects.join("-home-saidler-repos-foo");

    // Parent session.
    touch(&proj.join(format!("{UUID_A}.jsonl")), "{}\n");
    // Its subagents.
    touch(&proj.join(UUID_A).join("subagents").join("agent-1.jsonl"), "{}\n");
    touch(&proj.join(UUID_A).join("subagents").join("agent-2.jsonl"), "{}\n");
    // A second parent.
    touch(&proj.join(format!("{UUID_B}.jsonl")), "{}\n");
    // Noise that must be skipped.
    touch(&proj.join("not-a-uuid.jsonl"), "{}\n");
    touch(&proj.join(format!("{UUID_A}.jsonl.bak")), "ignored");
    touch(&proj.join("empty.jsonl"), ""); // non-uuid AND empty
    touch(&proj.join(format!("{}.jsonl", "empty-uuid")), ""); // non-uuid

    let files = find_session_files(projects).unwrap();
    let parents: Vec<_> = files.iter().filter(|f| f.kind == SessionFileKind::Parent).collect();
    let subs: Vec<_> = files.iter().filter(|f| f.kind == SessionFileKind::Subagent).collect();

    assert_eq!(parents.len(), 2, "two valid parents");
    assert_eq!(subs.len(), 2, "two subagents");
    assert!(
        subs.iter().all(|f| f.group_id == UUID_A),
        "subagents roll up to parent A"
    );
}

#[test]
fn empty_parent_file_is_skipped() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path();
    touch(&projects.join("proj").join(format!("{UUID_A}.jsonl")), "");
    assert!(find_session_files(projects).unwrap().is_empty());
}
