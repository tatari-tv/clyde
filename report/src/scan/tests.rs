#![allow(clippy::unwrap_used)]

use super::*;
use std::io::Write;
use tempfile::TempDir;

const PARENT_UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const PARENT_UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn write_jsonl(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    writeln!(f, "{}", body).unwrap();
}

fn touch_empty(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::File::create(path).unwrap();
}

#[test]
fn empty_dir_returns_no_files() {
    let tmp = TempDir::new().unwrap();
    let files = find_session_files(tmp.path()).unwrap();
    assert!(files.is_empty());
}

#[test]
fn nonexistent_dir_returns_no_files() {
    let files = find_session_files(Path::new("/nonexistent/cr-test/path")).unwrap();
    assert!(files.is_empty());
}

#[test]
fn parent_only_one_file() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-repos-foo-bar");
    let parent = project.join(format!("{}.jsonl", PARENT_UUID_A));
    write_jsonl(&parent, r#"{"type":"system"}"#);

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, parent);
    assert_eq!(files[0].group_id, PARENT_UUID_A);
    assert_eq!(files[0].kind, SessionFileKind::Parent);
}

#[test]
fn parent_with_subagents_rolled_up() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-repos-foo-bar");
    let parent = project.join(format!("{}.jsonl", PARENT_UUID_A));
    let agent = project
        .join(PARENT_UUID_A)
        .join("subagents")
        .join("agent-aabbccdd.jsonl");
    write_jsonl(&parent, r#"{"type":"assistant"}"#);
    write_jsonl(&agent, r#"{"type":"assistant"}"#);

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 2);
    let parent_file = files.iter().find(|f| f.kind == SessionFileKind::Parent).unwrap();
    let sub_file = files.iter().find(|f| f.kind == SessionFileKind::Subagent).unwrap();
    assert_eq!(parent_file.group_id, PARENT_UUID_A);
    assert_eq!(sub_file.group_id, PARENT_UUID_A);
    assert_eq!(parent_file.path, parent);
    assert_eq!(sub_file.path, agent);
}

#[test]
fn subagent_without_sibling_parent_is_kept() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-repos-foo-bar");
    let agent = project.join(PARENT_UUID_B).join("subagents").join("agent-orphan.jsonl");
    write_jsonl(&agent, r#"{"type":"assistant"}"#);

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, SessionFileKind::Subagent);
    assert_eq!(files[0].group_id, PARENT_UUID_B);
}

#[test]
fn non_jsonl_files_ignored() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    let parent = project.join(format!("{}.jsonl", PARENT_UUID_A));
    write_jsonl(&parent, r#"{"type":"system"}"#);
    write_jsonl(&project.join("notes.txt"), "hello");

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, parent);
}

#[test]
fn empty_jsonl_files_skipped() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    touch_empty(&project.join(format!("{}.jsonl", PARENT_UUID_A)));

    let files = find_session_files(tmp.path()).unwrap();
    assert!(files.is_empty());
}

#[test]
fn non_uuid_parent_stem_fails_loud() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    write_jsonl(&project.join("not-a-uuid.jsonl"), r#"{"type":"system"}"#);

    let err = find_session_files(tmp.path()).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("not a UUID-v4"), "expected loud failure, got: {}", msg);
}

#[test]
fn non_uuid_parent_dir_with_subagents_fails_loud() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    let agent = project.join("not-a-uuid").join("subagents").join("agent.jsonl");
    write_jsonl(&agent, r#"{"type":"assistant"}"#);

    let err = find_session_files(tmp.path()).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("not a UUID-v4"), "expected loud failure, got: {}", msg);
}
