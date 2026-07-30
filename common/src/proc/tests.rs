#![allow(clippy::unwrap_used)]

use super::*;

/// Bigger than the ~64KB OS pipe buffer in BOTH directions, which is the whole point: this payload
/// size is what deadlocks `run_bounded`'s pipe-and-post-exit-drain shape.
const OVER_PIPE_BUFFER: usize = 256 * 1024;

fn big_payload() -> String {
    // Non-trivial content so a truncation shows up as a length mismatch, not as equal blanks.
    "abcdefgh".repeat(OVER_PIPE_BUFFER / 8)
}

#[test]
fn run_with_payload_round_trips_a_payload_larger_than_the_pipe_buffer() {
    let payload = big_payload();
    assert!(payload.len() > 64 * 1024, "payload must exceed the pipe buffer");
    let mut cmd = Command::new("cat");
    // `cat` echoes stdin to stdout, so this drives a large payload IN and a large capture OUT
    // simultaneously — the exact pairing that deadlocks a pipe-based helper.
    let out = run_with_payload("cat (test)", &mut cmd, &payload, |e| eyre::eyre!("{e}")).unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), payload.len(), "stdout must not be truncated");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), payload);
}

#[test]
fn run_with_payload_captures_large_stdout_without_reading_stdin() {
    // A child that ignores stdin entirely and floods stdout. A post-exit pipe drain would deadlock
    // here once the child filled the buffer; a file cannot fill.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("yes abcdefghij | head -c 300000");
    let out = run_with_payload("flood (test)", &mut cmd, &big_payload(), |e| eyre::eyre!("{e}")).unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 300_000);
}

#[test]
fn run_with_payload_returns_nonzero_status_rather_than_erring() {
    // A non-zero exit is DATA for the caller (the cli transport reports it with observations), not an
    // error from the helper. If this ever became an Err, the transport could not build its report.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("exit 3");
    let out = run_with_payload("fail (test)", &mut cmd, "x", |e| eyre::eyre!("{e}")).unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn run_with_payload_captures_stderr_separately_from_stdout() {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("echo to-out; echo to-err >&2; exit 1");
    let out = run_with_payload("both (test)", &mut cmd, "x", |e| eyre::eyre!("{e}")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "to-out");
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "to-err");
}

#[test]
fn run_with_payload_delivers_stdin_from_byte_zero() {
    // Regression guard: handing the child the WRITE handle would leave it positioned at EOF, so the
    // child would see an empty stdin. Reading only the first line proves position 0, not just length.
    let mut cmd = Command::new("head");
    cmd.arg("-n").arg("1");
    let out = run_with_payload("head (test)", &mut cmd, "first\nsecond\n", |e| eyre::eyre!("{e}")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "first");
}

#[test]
fn run_with_payload_maps_a_spawn_failure_through_the_callers_closure() {
    let mut cmd = Command::new("definitely-not-a-real-binary-xyz");
    let err = run_with_payload("missing (test)", &mut cmd, "x", |e| {
        eyre::eyre!("caller-specific message: {e}")
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("caller-specific message"), "got: {err}");
}

#[test]
fn claude_timeout_is_far_wider_than_the_pandoc_ceiling() {
    // Phase 0 measured 145s (markdown) and 204s (html) on a real month, both over SUBPROCESS_TIMEOUT.
    // Reusing the 120s ceiling would have killed every real render, so the two must stay distinct.
    assert!(
        CLAUDE_TIMEOUT > SUBPROCESS_TIMEOUT,
        "the claude ceiling must exceed the pandoc/marquee one"
    );
    assert!(
        CLAUDE_TIMEOUT.as_secs() >= 4 * 204,
        "keep meaningful headroom over the 204s worst observed generation"
    );
}
