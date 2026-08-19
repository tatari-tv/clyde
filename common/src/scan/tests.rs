#![allow(clippy::unwrap_used)]

use super::*;
// The CRATE-level env lock, never a module-local one: `set_var`/`remove_var` mutate the whole
// environ block, so two modules each holding their own mutex would not serialize against each other
// (see the rationale on `crate::ENV_LOCK`).
use crate::ENV_LOCK;
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

// --- Claude Code orphan sidecars (the sanctioned exception to the fail-loud guard) ---

#[test]
fn orphan_sidecar_is_skipped_not_fatal() {
    // The real filename that took down `clyde cost`: a live session UUID plus Claude Code's
    // `.orphaned-<epoch-ms>-<hash>` suffix. It must not bail, and must not be discovered either.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    write_jsonl(
        &project.join(format!("{PARENT_UUID_A}.jsonl")),
        r#"{"type":"assistant"}"#,
    );
    write_jsonl(
        &project.join(format!("{PARENT_UUID_A}.orphaned-1786204682562-a5d5862b.jsonl")),
        r#"{"type":"ai-title","aiTitle":"t"}"#,
    );

    let files = find_session_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1, "only the live parent is discovered, got: {:?}", files);
    assert!(files[0].path.ends_with(format!("{PARENT_UUID_A}.jsonl")));
}

#[test]
fn orphan_sidecar_without_a_live_parent_is_still_skipped() {
    // The sidecar can outlive its parent transcript; on its own it is still not a session.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    write_jsonl(
        &project.join(format!("{PARENT_UUID_A}.orphaned-1786204682562-a5d5862b.jsonl")),
        r#"{"type":"agent-name","agentName":"n"}"#,
    );

    assert!(find_session_files(tmp.path()).unwrap().is_empty());
}

#[test]
fn orphan_sidecar_predicate_is_tight() {
    let uuid = PARENT_UUID_A;
    assert!(is_orphan_sidecar(&format!("{uuid}.orphaned-1786204682562-a5d5862b")));
    // A non-UUID prefix is not a sidecar -- it stays fatal, so corruption still surfaces.
    assert!(!is_orphan_sidecar("not-a-uuid.orphaned-1786204682562-a5d5862b"));
    // Near-misses must not open the escape hatch wider than the real artifact.
    assert!(!is_orphan_sidecar(uuid));
    assert!(!is_orphan_sidecar(&format!("{uuid}.orphaned")));
    assert!(!is_orphan_sidecar(&format!("{uuid}.orphaned-a5d5862b")));
    assert!(!is_orphan_sidecar(&format!(
        "{uuid}.orphaned-1786204682562-a5d5862b.extra"
    )));
}

#[test]
fn non_uuid_stem_that_is_not_a_sidecar_still_fails_loud() {
    // The escape hatch must not soften the guard: garbage is still fatal.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-home-saidler-foo");
    write_jsonl(&project.join("3a57f1ba.orphaned-1-ab.jsonl"), r#"{"type":"system"}"#);

    let err = find_session_files(tmp.path()).unwrap_err();
    assert!(format!("{:#}", err).contains("not a UUID-v4"));
}

#[test]
fn orphan_sidecar_shape_reports_lines_and_usage() {
    // The warning's whole job is to surface the day a sidecar carries real spend.
    let tmp = TempDir::new().unwrap();
    let metadata_only = tmp.path().join("meta.jsonl");
    write_jsonl(&metadata_only, r#"{"type":"ai-title","aiTitle":"t"}"#);
    assert_eq!(orphan_sidecar_shape(&metadata_only), (1, false));

    let with_usage = tmp.path().join("spend.jsonl");
    write_jsonl(
        &with_usage,
        "{\"type\":\"assistant\"}\n{\"usage\":{\"input_tokens\":7}}",
    );
    assert_eq!(orphan_sidecar_shape(&with_usage), (2, true));

    // An unreadable/absent path degrades to a benign shape rather than panicking mid-scan.
    assert_eq!(orphan_sidecar_shape(&tmp.path().join("absent.jsonl")), (0, false));
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

// --- Explicit-layout discovery, the pricing predicate, and the staged union (archived-session-spend
// Phase 1). `archived` records transcript availability, never "this spend did not happen", so the
// pricing path must resolve a session's bytes from wherever they are: live root first, staged
// second.

const PROJECT_DIR_NAME: &str = "-home-saidler-repos-foo-bar";

/// Build one live session under a projects tree: `<projects>/<project>/<id>.jsonl` plus
/// `<projects>/<project>/<id>/subagents/<name>`. Returns the parent transcript path.
fn live_session(projects: &Path, project: &str, id: &str, subagents: &[&str]) -> PathBuf {
    let project_dir = projects.join(project);
    let parent = project_dir.join(format!("{id}.jsonl"));
    write_jsonl(&parent, r#"{"type":"assistant"}"#);
    for name in subagents {
        write_jsonl(
            &project_dir.join(id).join("subagents").join(name),
            r#"{"type":"assistant"}"#,
        );
    }
    parent
}

/// Build one staged session dir: `<staged>/<id>/<id>.jsonl` plus `<staged>/<id>/subagents/<name>`.
/// `with_parent = false` builds the subagent-only shape `transcript_layout_parts` rejects.
fn staged_session(staged_root: &Path, id: &str, with_parent: bool, subagents: &[&str]) -> PathBuf {
    let dir = staged_root.join(id);
    fs::create_dir_all(&dir).unwrap();
    if with_parent {
        write_jsonl(&dir.join(format!("{id}.jsonl")), r#"{"type":"assistant"}"#);
    }
    for name in subagents {
        write_jsonl(&dir.join("subagents").join(name), r#"{"type":"assistant"}"#);
    }
    dir
}

#[test]
fn layout_files_collects_parent_and_every_subagent() {
    let tmp = TempDir::new().unwrap();
    let parent = live_session(
        tmp.path(),
        PROJECT_DIR_NAME,
        PARENT_UUID_A,
        &["agent-one.jsonl", "agent-two.jsonl"],
    );
    let subagents = tmp.path().join(PROJECT_DIR_NAME).join(PARENT_UUID_A).join("subagents");

    let files = layout_files(PARENT_UUID_A, &parent, &subagents);

    assert_eq!(files.len(), 3, "one parent plus two subagents");
    assert_eq!(files.iter().filter(|f| f.kind == SessionFileKind::Parent).count(), 1);
    assert_eq!(files.iter().filter(|f| f.kind == SessionFileKind::Subagent).count(), 2);
    assert!(
        files.iter().all(|f| f.group_id == PARENT_UUID_A),
        "subagent spend must fold into the parent session's group"
    );
    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "layout discovery must be path-sorted");
}

#[test]
fn layout_files_subagents_without_parent_is_non_empty() {
    // The case `transcript_layout_parts` rejects: no parent transcript, but the subagent files hold
    // real usage records, so pricing must still see them.
    let tmp = TempDir::new().unwrap();
    let dir = staged_session(tmp.path(), PARENT_UUID_A, false, &["agent-one.jsonl"]);

    let files = layout_files(
        PARENT_UUID_A,
        &dir.join(format!("{PARENT_UUID_A}.jsonl")),
        &dir.join("subagents"),
    );

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, SessionFileKind::Subagent);
    assert_eq!(files[0].group_id, PARENT_UUID_A);
}

#[test]
fn layout_files_absent_layout_is_empty() {
    let tmp = TempDir::new().unwrap();
    let files = layout_files(
        PARENT_UUID_A,
        &tmp.path().join(format!("{PARENT_UUID_A}.jsonl")),
        &tmp.path().join("subagents"),
    );
    assert!(files.is_empty(), "no bytes anywhere means an empty vec");
}

#[test]
fn layout_files_skips_empty_files() {
    // Shares `make_parent`/`make_subagent` with `find_session_files`, so the empty-file skip is the
    // same rule rather than a reimplementation.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(PARENT_UUID_A);
    touch_empty(&dir.join(format!("{PARENT_UUID_A}.jsonl")));
    touch_empty(&dir.join("subagents").join("agent-one.jsonl"));

    let files = layout_files(
        PARENT_UUID_A,
        &dir.join(format!("{PARENT_UUID_A}.jsonl")),
        &dir.join("subagents"),
    );
    assert!(files.is_empty(), "zero-byte transcripts carry no usage records");
}

#[test]
fn pricing_files_prefers_live_when_both_roots_hold_the_session() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    let parent = live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &["agent-one.jsonl"]);
    let staged = staged_session(&staged_root, PARENT_UUID_A, true, &["agent-one.jsonl"]);

    let files = pricing_files(
        PARENT_UUID_A,
        &parent,
        &projects.join(PROJECT_DIR_NAME),
        Some(staged.as_path()),
    );

    assert_eq!(files.len(), 2, "the live layout only, never both roots' copies");
    assert!(
        files.iter().all(|f| f.path.starts_with(&projects)),
        "every resolved path must be under the live root: {:?}",
        files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
    );
    let groups: BTreeSet<&str> = files.iter().map(|f| f.group_id.as_str()).collect();
    assert_eq!(groups.len(), 1, "a both-roots session yields exactly one group");
}

#[test]
fn pricing_files_falls_back_to_the_staged_copy() {
    // The archived case: the live transcript is reaped, the staged copy holds the money.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    let staged = staged_session(
        &staged_root,
        PARENT_UUID_A,
        true,
        &["agent-one.jsonl", "agent-two.jsonl"],
    );
    let reaped_parent = projects.join(PROJECT_DIR_NAME).join(format!("{PARENT_UUID_A}.jsonl"));

    let files = pricing_files(
        PARENT_UUID_A,
        &reaped_parent,
        &projects.join(PROJECT_DIR_NAME),
        Some(staged.as_path()),
    );

    assert_eq!(files.len(), 3, "the staged parent plus every staged subagent");
    assert!(files.iter().all(|f| f.path.starts_with(&staged_root)));
    assert!(files.iter().all(|f| f.group_id == PARENT_UUID_A));
}

#[test]
fn pricing_files_accepts_a_staged_session_with_subagents_but_no_parent() {
    // Recoverability is "does `pricing_files` return bytes", NOT "does the body resolver return
    // Some". A session whose parent was reaped between reconcile and staging still has spend.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    let staged = staged_session(&staged_root, PARENT_UUID_A, false, &["agent-one.jsonl"]);
    let reaped_parent = projects.join(PROJECT_DIR_NAME).join(format!("{PARENT_UUID_A}.jsonl"));

    let files = pricing_files(
        PARENT_UUID_A,
        &reaped_parent,
        &projects.join(PROJECT_DIR_NAME),
        Some(staged.as_path()),
    );

    assert_eq!(files.len(), 1, "subagent-only staged bytes are still priceable");
    assert_eq!(files[0].kind, SessionFileKind::Subagent);
}

#[test]
fn pricing_files_is_empty_when_there_are_no_bytes_anywhere() {
    // The only unrecoverable state, and the one Phase 2/Phase 3 both branch on.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged = tmp.path().join("staged").join(PARENT_UUID_A);
    fs::create_dir_all(&staged).unwrap();

    let files = pricing_files(
        PARENT_UUID_A,
        &projects.join(PROJECT_DIR_NAME).join(format!("{PARENT_UUID_A}.jsonl")),
        &projects.join(PROJECT_DIR_NAME),
        Some(staged.as_path()),
    );
    assert!(files.is_empty());
}

#[test]
fn pricing_files_is_empty_when_there_is_no_staged_path_at_all() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    let files = pricing_files(
        PARENT_UUID_A,
        &projects.join(PROJECT_DIR_NAME).join(format!("{PARENT_UUID_A}.jsonl")),
        &projects.join(PROJECT_DIR_NAME),
        None,
    );
    assert!(files.is_empty());
}

#[test]
fn staged_union_admits_a_staged_only_session() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &[]);
    staged_session(&staged_root, PARENT_UUID_B, true, &["agent-one.jsonl"]);

    let files = find_session_files_with_staged(&projects, &staged_root).unwrap();

    let groups: BTreeSet<&str> = files.iter().map(|f| f.group_id.as_str()).collect();
    assert!(groups.contains(PARENT_UUID_A), "the live session stays");
    assert!(groups.contains(PARENT_UUID_B), "the staged-only session is admitted");
    let staged_only: Vec<&SessionFile> = files.iter().filter(|f| f.group_id == PARENT_UUID_B).collect();
    assert_eq!(staged_only.len(), 2, "its parent and every subagent file");
}

#[test]
fn staged_union_counts_a_both_roots_session_exactly_once() {
    // Live-then-staged precedence is the ONLY thing preventing a double count on the efficiency
    // surfaces, whose dedup is per-file. 94 sessions on desk.lan have bytes in both roots today.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &["agent-one.jsonl"]);
    staged_session(&staged_root, PARENT_UUID_A, true, &["agent-one.jsonl"]);

    let files = find_session_files_with_staged(&projects, &staged_root).unwrap();

    assert_eq!(files.len(), 2, "the live copy only, not four files");
    assert!(
        files.iter().all(|f| f.path.starts_with(&projects)),
        "precedence must resolve to the live root"
    );
}

#[test]
fn staged_union_with_a_nonexistent_staged_root_equals_the_live_scan() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &["agent-one.jsonl"]);

    let live_only = find_session_files(&projects).unwrap();
    let unioned = find_session_files_with_staged(&projects, &tmp.path().join("no-such-staged")).unwrap();

    let live_paths: Vec<PathBuf> = live_only.iter().map(|f| f.path.clone()).collect();
    let union_paths: Vec<PathBuf> = unioned.iter().map(|f| f.path.clone()).collect();
    assert_eq!(union_paths, live_paths);
}

#[test]
fn staged_union_warns_and_skips_a_non_uuid_staged_dir() {
    // Deliberate asymmetry with `find_session_files`, which bails: one stray directory in a
    // clyde-owned cache must not brick every `clyde cost` invocation.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &[]);
    write_jsonl(
        &staged_root.join("not-a-uuid").join("not-a-uuid.jsonl"),
        r#"{"type":"assistant"}"#,
    );
    staged_session(&staged_root, PARENT_UUID_B, true, &[]);

    let files = find_session_files_with_staged(&projects, &staged_root).unwrap();

    let groups: BTreeSet<&str> = files.iter().map(|f| f.group_id.as_str()).collect();
    assert_eq!(groups.len(), 2, "the stray dir is skipped, the valid ones survive");
    assert!(groups.contains(PARENT_UUID_A) && groups.contains(PARENT_UUID_B));
}

#[test]
fn staged_union_is_sorted_by_path() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let staged_root = tmp.path().join("staged");
    live_session(&projects, PROJECT_DIR_NAME, PARENT_UUID_A, &["agent-one.jsonl"]);
    staged_session(&staged_root, PARENT_UUID_B, true, &["agent-two.jsonl"]);

    let files = find_session_files_with_staged(&projects, &staged_root).unwrap();

    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "the union must keep the stable-order contract");
}

#[test]
fn default_staged_dir_honors_xdg_data_home() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };
    assert_eq!(
        default_staged_dir(),
        Some(dir.path().join("clyde").join("staged")),
        "the staged root follows $XDG_DATA_HOME on every platform"
    );

    unsafe { std::env::remove_var("XDG_DATA_HOME") };
    assert!(
        default_staged_dir().unwrap().ends_with(".local/share/clyde/staged"),
        "unset falls back to $HOME/.local/share, never ~/Library/..."
    );

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
