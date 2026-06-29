//! Integration test for `report collect` stdout streaming (Phase 6, #10b).
//!
//! Driven through the real `cr` binary so stdout and stderr are genuine, separable streams —
//! the only way to prove HAZARD 1 (the "wrote N sessions" note must NOT corrupt the JSON on
//! stdout) end to end.

use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

#[test]
fn stdout_mode_streams_valid_json_and_message_to_stderr() {
    // HAZARD 1: when `-o` is omitted, the JSON streams to stdout and the "wrote N sessions"
    // note must land on STDERR, never stdout — otherwise it corrupts the JSON a `| jq` consumes.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("-home-saidler-repos-foo");
    write_jsonl(
        &project.join(format!("{}.jsonl", SID_A)),
        &[
            r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-10T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ],
    );

    let bin = env!("CARGO_BIN_EXE_cr");
    let out = std::process::Command::new(bin)
        .args([
            "collect",
            "--skip-title",
            "--since",
            "2026-01-01",
            "--until",
            "2030-01-01",
            "--projects-dir",
        ])
        .arg(&projects)
        .output()
        .expect("cr collect should run");

    assert!(out.status.success(), "cr collect exited non-zero: {:?}", out.status);

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();

    // stdout is pure JSON: it must parse, expose the expected session, and NOT carry the note.
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid report JSON");
    assert_eq!(value["totals"]["sessions"], 1);
    assert!(value["sessions"].get(SID_A).is_some());
    assert!(
        !stdout.contains("wrote"),
        "the 'wrote N sessions' note must NOT appear on stdout (would corrupt the JSON stream)"
    );

    // The note belongs on stderr instead.
    assert!(
        stderr.contains("wrote 1 sessions to stdout"),
        "expected the 'wrote N' note on stderr, got: {stderr:?}"
    );
}
