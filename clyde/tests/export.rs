//! Integration tests for `clyde session export`, driven through the real `clyde` binary end to
//! end (the only way to prove the CLI wiring, not just the underlying `Db::export`/`export_one`
//! query already covered by `sessions`' own tests).
//!
//! Covers the Phase 3 success criteria from the design doc:
//! - the emitted envelope validates structurally against the Phase 0 golden fixtures (field set,
//!   not just `jq .`)
//! - an empty `--cursor` result echoes the request cursor
//! - two `--limit` pages concatenate with no gap and no overlap
//! - an unknown `--id` exits nonzero

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
const SID_C: &str = "7c19b25e-0d11-4e4b-a82d-2345678901bc";

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

/// One minimal, valid parent-transcript line for `sid` under `cwd`, timestamped `ts`.
fn seed_session(projects: &Path, project_dir: &str, sid: &str, cwd: &str, ts: &str) {
    write_jsonl(
        &projects.join(project_dir).join(format!("{sid}.jsonl")),
        &[&format!(
            r#"{{"type":"user","cwd":"{cwd}","gitBranch":"main","timestamp":"{ts}","message":{{"content":"hello from {sid}"}}}}"#
        )],
    );
}

fn seed_projects(projects: &Path) {
    seed_session(
        projects,
        "-home-saidler-repos-tatari-tv-marquee",
        SID_A,
        "/home/saidler/repos/tatari-tv/marquee",
        "2026-06-21T10:00:00Z",
    );
    seed_session(
        projects,
        "-home-saidler-repos-tatari-tv-loopr",
        SID_B,
        "/home/saidler/repos/tatari-tv/loopr",
        "2026-06-22T10:00:00Z",
    );
    seed_session(
        projects,
        "-home-saidler-repos-tatari-tv-clyde",
        SID_C,
        "/home/saidler/repos/tatari-tv/clyde",
        "2026-06-23T10:00:00Z",
    );
}

fn reindex(bin: &str, db_path: &Path, projects: &Path) {
    let out = std::process::Command::new(bin)
        .arg("--db")
        .arg(db_path)
        .args(["session", "reindex", "--projects-dir"])
        .arg(projects)
        .output()
        .expect("clyde session reindex should run");
    assert!(out.status.success(), "reindex failed: {:?}", out);
}

/// Run `clyde session export` with `extra_args`, asserting a clean exit, and parse stdout as JSON.
///
/// Always passes `--no-reindex`: the caller has already reindexed against the seeded temp
/// projects dir, and `cmd_export`'s own lazy reindex (like `search`/`ls`) targets the REAL
/// `~/.claude/projects` when not skipped, which would pollute the test DB with the operator's
/// live catalog.
fn run_export(bin: &str, db_path: &Path, extra_args: &[&str]) -> serde_json::Value {
    let mut args = vec!["--db".to_string(), db_path.to_string_lossy().into_owned()];
    args.extend(["session".to_string(), "export".to_string(), "--no-reindex".to_string()]);
    args.extend(extra_args.iter().map(|s| s.to_string()));

    let out = std::process::Command::new(bin)
        .args(&args)
        .output()
        .expect("clyde session export should run");
    assert!(out.status.success(), "export failed: {:?}", out);

    let stdout = String::from_utf8(out.stdout).expect("stdout is valid utf8");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout:?}"))
}

/// The Phase 0 golden fixture at `sessions/tests/fixtures/export/<name>`, loaded as a `Value`.
/// `sessions` is a sibling crate to `clyde`, so the path is relative to `CARGO_MANIFEST_DIR`.
fn fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sessions/tests/fixtures/export")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
}

/// Key set of a JSON object, one level deep — enough to structurally pin the envelope/record
/// shape (renamed, dropped, or added fields all change this set) without pinning exact values.
fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {v}"))
        .keys()
        .cloned()
        .collect()
}

#[test]
fn bulk_export_envelope_matches_fixture_schema() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);
    let bin = env!("CARGO_BIN_EXE_clyde");
    reindex(bin, &db_path, &projects);

    let envelope = run_export(bin, &db_path, &["--include-archived"]);

    // Envelope-level shape: same top-level keys as every Phase 0 fixture.
    let fixture_envelope = fixture("never-enriched.json");
    assert_eq!(
        keys(&envelope),
        keys(&fixture_envelope),
        "envelope top-level keys diverged from the Phase 0 fixture: {envelope}"
    );

    let sessions = envelope["sessions"].as_array().expect("sessions is an array");
    assert_eq!(
        sessions.len(),
        3,
        "all three seeded sessions should be exported: {envelope}"
    );

    // Per-record shape: metadata-mode records (no `--with-body`) carry exactly the never-enriched
    // fixture's field set — no body-block keys.
    let fixture_record = &fixture_envelope["sessions"][0];
    for rec in sessions {
        assert_eq!(
            keys(rec),
            keys(fixture_record),
            "exported record keys diverged from the Phase 0 fixture: {rec}"
        );
    }
}

#[test]
fn with_body_export_matches_fixture_schema() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);
    let bin = env!("CARGO_BIN_EXE_clyde");
    reindex(bin, &db_path, &projects);

    let envelope = run_export(bin, &db_path, &["--id", SID_A, "--with-body"]);
    let sessions = envelope["sessions"].as_array().expect("sessions is an array");
    assert_eq!(sessions.len(), 1, "--id returns exactly one record: {envelope}");
    assert_eq!(sessions[0]["session-id"], SID_A);

    let fixture_envelope = fixture("with-body.json");
    let fixture_record = &fixture_envelope["sessions"][0];
    assert_eq!(
        keys(&sessions[0]),
        keys(fixture_record),
        "--with-body record keys diverged from the Phase 0 with-body fixture: {}",
        sessions[0]
    );
    assert!(
        sessions[0]["body"].is_array(),
        "body should be a parsed array: {}",
        sessions[0]
    );
    assert_eq!(sessions[0]["body-error"], serde_json::Value::Null);
}

#[test]
fn empty_cursor_result_echoes_the_request_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);
    let bin = env!("CARGO_BIN_EXE_clyde");
    reindex(bin, &db_path, &projects);

    // A cursor past every seeded session's revision (3 rows -> revisions 1..3) yields an empty
    // page whose `cursor` echoes back the request, never 0 or the max seen.
    let envelope = run_export(bin, &db_path, &["--cursor", "999", "--include-archived"]);
    assert_eq!(
        envelope["sessions"].as_array().unwrap().len(),
        0,
        "expected an empty page: {envelope}"
    );
    assert_eq!(
        envelope["cursor"], 999,
        "empty result must echo the request cursor: {envelope}"
    );
}

#[test]
fn limit_paging_has_no_gap_and_no_overlap() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);
    let bin = env!("CARGO_BIN_EXE_clyde");
    reindex(bin, &db_path, &projects);

    let page1 = run_export(bin, &db_path, &["--limit", "2", "--include-archived"]);
    let page1_sessions = page1["sessions"].as_array().unwrap();
    assert_eq!(page1_sessions.len(), 2, "page 1 should honor --limit: {page1}");
    let cursor1 = page1["cursor"].as_i64().expect("cursor is an integer");

    let cursor_arg = cursor1.to_string();
    let page2 = run_export(
        bin,
        &db_path,
        &["--limit", "2", "--cursor", &cursor_arg, "--include-archived"],
    );
    let page2_sessions = page2["sessions"].as_array().unwrap();
    assert_eq!(
        page2_sessions.len(),
        1,
        "page 2 should hold the one remaining session: {page2}"
    );

    let ids1: BTreeSet<&str> = page1_sessions
        .iter()
        .map(|r| r["session-id"].as_str().unwrap())
        .collect();
    let ids2: BTreeSet<&str> = page2_sessions
        .iter()
        .map(|r| r["session-id"].as_str().unwrap())
        .collect();

    assert!(
        ids1.is_disjoint(&ids2),
        "pages must not overlap: page1={ids1:?} page2={ids2:?}"
    );
    let union: BTreeSet<&str> = ids1.union(&ids2).cloned().collect();
    let all: BTreeSet<&str> = [SID_A, SID_B, SID_C].into_iter().collect();
    assert_eq!(
        union, all,
        "pages must cover every session with no gap: page1={ids1:?} page2={ids2:?}"
    );

    // Every revision in page 2 is strictly greater than page 1's cursor (the `>` semantics that
    // guarantee no gap): nothing between cursor1 and the next page's rows was skipped.
    for rec in page2_sessions {
        let rev = rec["updated-at"].as_i64().expect("updated-at is an integer");
        assert!(
            rev > cursor1,
            "page 2 revision {rev} must exceed page 1's cursor {cursor1}"
        );
    }
}

#[test]
fn unknown_id_exits_nonzero() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);
    let bin = env!("CARGO_BIN_EXE_clyde");
    reindex(bin, &db_path, &projects);

    let out = std::process::Command::new(bin)
        .arg("--db")
        .arg(&db_path)
        .args([
            "session",
            "export",
            "--no-reindex",
            "--id",
            "00000000-0000-0000-0000-000000000000",
        ])
        .output()
        .expect("clyde session export --id should run");

    assert!(
        !out.status.success(),
        "an unknown --id must exit nonzero: {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no session matches"),
        "expected a 'no session matches' message on stderr, got: {stderr}"
    );
}
