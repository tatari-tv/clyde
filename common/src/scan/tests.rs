#![allow(clippy::unwrap_used)]

use super::*;
use chrono::TimeZone;
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

// --- Discovery + grouping (harvested from report/src/scan/tests.rs) ---

#[test]
fn empty_dir_returns_no_files() {
    let tmp = TempDir::new().unwrap();
    let files = find_session_files(tmp.path()).unwrap();
    assert!(files.is_empty());
}

#[test]
fn nonexistent_dir_returns_no_files() {
    let files = find_session_files(Path::new("/nonexistent/scan-test/path")).unwrap();
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
    // Unified fields populated from the single fs::metadata call.
    assert!(files[0].size > 0, "size must be read from metadata");
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
fn tool_results_dir_ignored() {
    // A session-uuid dir with NO subagents/ (only a tool-results/ dir) contributes nothing, and
    // must not trip the UUID guard's subagents branch.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    let tool_results = project.join(PARENT_UUID_A).join("tool-results");
    write_jsonl(&tool_results.join("output.jsonl"), r#"{"type":"assistant"}"#);

    let files = find_session_files(tmp.path()).unwrap();
    assert!(files.is_empty(), "only subagents/ is traversed, not tool-results/");
}

// --- Fail-loud UUID-v4 guard (Phase 5 success criterion) ---

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
fn non_uuid_subagent_dir_fails_loud() {
    // AC: "a malformed non-UUID subagent dir triggers bail!" -- a session directory carrying a
    // subagents/ folder whose own name is not a UUID-v4 must fail loud, not be misclassified.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    let agent = project.join("not-a-uuid").join("subagents").join("agent.jsonl");
    write_jsonl(&agent, r#"{"type":"assistant"}"#);

    let err = find_session_files(tmp.path()).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("not a UUID-v4"), "expected loud failure, got: {}", msg);
}

// --- Deterministic path sorting (harvested from cost/src/scanner/tests.rs) ---

#[test]
fn discovery_is_sorted_by_path() {
    // Discovery must return a path-sorted list so insertion order into the parse/dedup pipeline is
    // deterministic regardless of read_dir's filesystem order. Distinct UUID stems, created in a
    // deliberately non-sorted order across project dirs.
    let tmp = TempDir::new().unwrap();
    let stems = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
    ];
    for (project, stem) in ["zeta", "alpha", "mike", "bravo"].iter().zip(stems) {
        let project_dir = tmp.path().join(format!("proj-{project}"));
        let jsonl = project_dir.join(format!("{stem}.jsonl"));
        write_jsonl(&jsonl, r#"{"type":"assistant"}"#);
    }

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 4);
    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    let mut expected = paths.clone();
    expected.sort();
    assert_eq!(paths, expected, "discovery must return path-sorted files");
}

// --- mtime lower-bound prefilter (harvested from cost/src/scanner/tests.rs) ---

/// Build a SessionFile whose mtime falls on `date` (local noon, DST-safe), for prefilter tests.
fn session_file_with_mtime_date(path: &str, date: NaiveDate) -> SessionFile {
    let dt = chrono::Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).expect("valid time"))
        .single()
        .expect("unambiguous local time");
    SessionFile {
        path: PathBuf::from(path),
        group_id: "group".to_string(),
        kind: SessionFileKind::Parent,
        mtime: dt.into(),
        size: 1,
    }
}

#[test]
fn filter_keeps_file_touched_after_end() {
    // A file whose mtime is out of range on the high side (touched after `end`, e.g. a
    // still-growing session queried for an earlier day) must survive the prefilter, because it can
    // still hold an in-window entry. A `mtime <= end` upper bound would silently drop these.
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let stale = session_file_with_mtime_date("after-end.jsonl", NaiveDate::from_ymd_opt(2026, 7, 5).expect("date"));
    let files = vec![stale];

    let kept = filter_by_date_range(&files, start, end);
    assert_eq!(kept.len(), 1, "a file touched after `end` must NOT be dropped");
    assert_eq!(kept[0].path, PathBuf::from("after-end.jsonl"));
}

#[test]
fn filter_keeps_in_window_file() {
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 10).expect("date");
    let f = session_file_with_mtime_date("in-window.jsonl", NaiveDate::from_ymd_opt(2026, 7, 5).expect("date"));
    let files = vec![f];

    let kept = filter_by_date_range(&files, start, end);
    assert_eq!(kept.len(), 1);
}

#[test]
fn filter_drops_file_before_start() {
    // The lower bound is still a valid optimization: under the append-only invariant a file whose
    // mtime is before `start` has every entry before `start` and holds no in-window content.
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 10).expect("date");
    let f = session_file_with_mtime_date("too-old.jsonl", NaiveDate::from_ymd_opt(2026, 6, 20).expect("date"));
    let files = vec![f];

    let kept = filter_by_date_range(&files, start, end);
    assert!(kept.is_empty(), "a file whose mtime precedes `start` is safely dropped");
}
