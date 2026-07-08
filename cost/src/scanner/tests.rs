#![allow(clippy::unwrap_used)]

use super::*;
use chrono::TimeZone;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_find_session_files() {
    let tmp = TempDir::new().expect("create temp dir");
    let project_dir = tmp.path().join("test-project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    // Create a JSONL file with content
    let jsonl_path = project_dir.join("session-123.jsonl");
    let mut file = fs::File::create(&jsonl_path).expect("create jsonl");
    writeln!(file, r#"{{"type":"system"}}"#).expect("write");

    // Create an empty file (should be skipped)
    fs::File::create(project_dir.join("empty.jsonl")).expect("create empty");

    // Create a non-JSONL file (should be skipped)
    let mut txt = fs::File::create(project_dir.join("notes.txt")).expect("create txt");
    writeln!(txt, "hello").expect("write");

    let files = find_session_files(tmp.path()).expect("find files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, jsonl_path);
}

#[test]
fn test_find_session_files_empty_dir() {
    let tmp = TempDir::new().expect("create temp dir");
    let files = find_session_files(tmp.path()).expect("find files");
    assert!(files.is_empty());
}

#[test]
fn test_find_session_files_nonexistent() {
    let files = find_session_files(Path::new("/nonexistent/path")).expect("find files");
    assert!(files.is_empty());
}

#[test]
fn test_find_session_files_includes_subagents() {
    let tmp = TempDir::new().expect("create temp dir");
    let project_dir = tmp.path().join("test-project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    // Direct session JSONL
    let parent_jsonl = project_dir.join("abc123.jsonl");
    let mut f = fs::File::create(&parent_jsonl).expect("create parent jsonl");
    writeln!(f, r#"{{"type":"system"}}"#).expect("write");

    // Subagent JSONL at <uuid>/subagents/<agent>.jsonl
    let subagents_dir = project_dir.join("abc123").join("subagents");
    fs::create_dir_all(&subagents_dir).expect("create subagents dir");
    let agent_jsonl = subagents_dir.join("agent-aabbccdd.jsonl");
    let mut f = fs::File::create(&agent_jsonl).expect("create agent jsonl");
    writeln!(f, r#"{{"type":"assistant"}}"#).expect("write");

    // Empty subagent file (should be skipped)
    fs::File::create(subagents_dir.join("agent-empty.jsonl")).expect("create empty");

    let files = find_session_files(tmp.path()).expect("find files");
    assert_eq!(files.len(), 2);
    let paths: Vec<_> = files.iter().map(|f| &f.path).collect();
    assert!(paths.contains(&&parent_jsonl));
    assert!(paths.contains(&&agent_jsonl));
}

#[test]
fn test_find_session_files_subagents_no_parent_jsonl() {
    let tmp = TempDir::new().expect("create temp dir");
    let project_dir = tmp.path().join("test-project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    // Subagent JSONL exists without a sibling parent .jsonl
    let subagents_dir = project_dir.join("orphan-uuid").join("subagents");
    fs::create_dir_all(&subagents_dir).expect("create subagents dir");
    let agent_jsonl = subagents_dir.join("agent-orphan.jsonl");
    let mut f = fs::File::create(&agent_jsonl).expect("create agent jsonl");
    writeln!(f, r#"{{"type":"assistant"}}"#).expect("write");

    let files = find_session_files(tmp.path()).expect("find files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, agent_jsonl);
}

#[test]
fn test_find_session_files_tool_results_ignored() {
    let tmp = TempDir::new().expect("create temp dir");
    let project_dir = tmp.path().join("test-project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    // tool-results dir should not be traversed for JSONL
    let tool_results_dir = project_dir.join("abc123").join("tool-results");
    fs::create_dir_all(&tool_results_dir).expect("create tool-results dir");
    let mut f = fs::File::create(tool_results_dir.join("output.json")).expect("create json");
    writeln!(f, "{{}}").expect("write");

    // Only the subagents dir should be scanned; tool-results has no JSONL so result is 0
    let files = find_session_files(tmp.path()).expect("find files");
    assert!(files.is_empty());
}

#[test]
fn test_find_session_files_sorted_by_path() {
    // Phase 1: discovery must return a path-sorted list so insertion order into the
    // parse/dedup pipeline is deterministic regardless of read_dir's filesystem order.
    let tmp = TempDir::new().expect("create temp dir");

    // Create several project dirs / files whose names are deliberately not in creation order.
    for name in ["zeta", "alpha", "mike", "bravo"] {
        let project_dir = tmp.path().join(format!("proj-{name}"));
        fs::create_dir_all(&project_dir).expect("create project dir");
        let jsonl = project_dir.join(format!("{name}.jsonl"));
        let mut f = fs::File::create(&jsonl).expect("create jsonl");
        writeln!(f, r#"{{"type":"assistant"}}"#).expect("write");
    }

    let files = find_session_files(tmp.path()).expect("find files");
    assert_eq!(files.len(), 4);
    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    let mut expected = paths.clone();
    expected.sort();
    assert_eq!(paths, expected, "discovery must return path-sorted files");
}

/// Build a SessionFile whose mtime falls on `date` (local noon, DST-safe), for prefilter tests.
fn session_file_with_mtime_date(path: &str, date: NaiveDate) -> SessionFile {
    let dt = chrono::Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).expect("valid time"))
        .single()
        .expect("unambiguous local time");
    SessionFile {
        path: PathBuf::from(path),
        mtime: dt.into(),
        size: 1,
    }
}

#[test]
fn test_filter_keeps_file_touched_after_end() {
    // Phase 1 correctness guarantee: a file whose mtime is OUT OF RANGE on the high side
    // (touched after `end`, e.g. a still-growing session queried for an earlier day) must
    // survive the prefilter, because it can still hold an in-window entry. The old
    // `mtime <= end` upper bound silently dropped these in-window dollars.
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let stale = session_file_with_mtime_date("after-end.jsonl", NaiveDate::from_ymd_opt(2026, 7, 5).expect("date"));
    let files = vec![stale];

    let kept = filter_by_date_range(&files, start, end);
    assert_eq!(kept.len(), 1, "a file touched after `end` must NOT be dropped");
    assert_eq!(kept[0].path, PathBuf::from("after-end.jsonl"));
}

#[test]
fn test_filter_keeps_in_window_file() {
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 10).expect("date");
    let f = session_file_with_mtime_date("in-window.jsonl", NaiveDate::from_ymd_opt(2026, 7, 5).expect("date"));
    let files = vec![f];

    let kept = filter_by_date_range(&files, start, end);
    assert_eq!(kept.len(), 1);
}

#[test]
fn test_filter_drops_file_before_start() {
    // The lower bound is still a valid optimization: under the append-only invariant a file
    // whose mtime is before `start` has every entry before `start` and holds no in-window
    // content, so dropping it loses nothing.
    let start = NaiveDate::from_ymd_opt(2026, 7, 1).expect("date");
    let end = NaiveDate::from_ymd_opt(2026, 7, 10).expect("date");
    let f = session_file_with_mtime_date("too-old.jsonl", NaiveDate::from_ymd_opt(2026, 6, 20).expect("date"));
    let files = vec![f];

    let kept = filter_by_date_range(&files, start, end);
    assert!(kept.is_empty(), "a file whose mtime precedes `start` is safely dropped");
}
