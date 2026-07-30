#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- envelope parsing: the two stdout-contamination guards -------------------------------------

#[test]
fn parse_envelope_reads_a_clean_envelope() {
    let env = parse_envelope(good_envelope().as_bytes()).unwrap();
    assert!(!env.is_error);
    assert_eq!(env.subtype.as_deref(), Some("success"));
    assert_eq!(env.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(env.usage.unwrap().output_tokens, Some(217));
}

#[test]
fn parse_envelope_tolerates_leading_noise_before_the_json_root() {
    // An npm update notice ahead of the JSON must NOT misreport an already-billed success as
    // malformed. Proven to bite: remove the first-`{` seek and this fails.
    let noisy = format!("╭─ update available: 2.1.220 ─╮\n\n{}", good_envelope());
    let env = parse_envelope(noisy.as_bytes()).unwrap();
    assert_eq!(env.stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn parse_envelope_bails_when_stdout_has_no_json_at_all() {
    let err = parse_envelope(b"not logged in\n").unwrap_err().to_string();
    assert!(err.contains("no JSON envelope"), "got: {err}");
    assert!(
        err.contains("check the install and login"),
        "must carry the escape hatch: {err}"
    );
}

#[test]
fn parse_envelope_bails_on_malformed_json() {
    let err = parse_envelope(b"{\"is_error\": tru").unwrap_err().to_string();
    assert!(err.contains("failed to parse"), "got: {err}");
}

#[test]
fn parse_envelope_bails_on_non_utf8_stdout() {
    // Lossy decoding is banned here: the envelope carries the artifact, so a replacement char would
    // corrupt the document we are about to publish.
    let err = parse_envelope(&[b'{', 0xff, 0xfe]).unwrap_err().to_string();
    assert!(err.contains("non-UTF-8"), "got: {err}");
}

#[test]
fn parse_envelope_ignores_unknown_fields() {
    // Forward-compatible envelope carve-out: it is a wire frame owned by another tool.
    let json = r#"{"is_error":false,"subtype":"success","stop_reason":"end_turn","result":"x",
                   "a_brand_new_field_from_a_future_cli":{"nested":true}}"#;
    let env = parse_envelope(json.as_bytes()).unwrap();
    assert_eq!(env.result.as_deref(), Some("x"));
}

// ---- AC12: the model guard is a keyed lookup, not a scan ---------------------------------------

#[test]
fn check_model_passes_on_a_real_multi_entry_envelope() {
    // THE regression test for Phase 0 finding F2. The CLI makes an internal haiku sub-call, so a
    // scan-and-compare-all would bail on every successful render. Proven to bite: change check_model
    // to iterate asserting all entries match and this fails.
    let env = parse_envelope(good_envelope().as_bytes()).unwrap();
    check_model(&env.model_usage, MODEL, OBS).expect("a haiku sub-call alongside the pin must not bail");
}

#[test]
fn check_model_matches_a_dated_key_against_an_undated_pin() {
    // The CLI keys entries by the dated id; the configured pin is usually undated.
    let usage = r#"{"claude-haiku-4-5-20251001":{"canonicalModel":"claude-haiku-4-5"}}"#;
    let env = parse_envelope(envelope_json(false, "success", "end_turn", "x", 1, usage).as_bytes()).unwrap();
    check_model(&env.model_usage, "claude-haiku-4-5", OBS).expect("dated key must match an undated pin");
}

#[test]
fn check_model_bails_when_the_pinned_model_is_absent() {
    let usage = r#"{"claude-haiku-4-5-20251001":{"canonicalModel":"claude-haiku-4-5"}}"#;
    let env = parse_envelope(envelope_json(false, "success", "end_turn", "x", 1, usage).as_bytes()).unwrap();
    let err = check_model(&env.model_usage, MODEL, OBS).unwrap_err().to_string();
    assert!(err.contains("no usage for the requested model"), "got: {err}");
    assert!(
        err.contains("claude-haiku-4-5-20251001"),
        "should name what DID run: {err}"
    );
}

#[test]
fn check_model_bails_when_canonical_model_was_substituted() {
    let usage = r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#;
    let env = parse_envelope(envelope_json(false, "success", "end_turn", "x", 1, usage).as_bytes()).unwrap();
    let err = check_model(&env.model_usage, MODEL, OBS).unwrap_err().to_string();
    assert!(err.contains("substituted model"), "got: {err}");
    assert!(err.contains("claude-sonnet-5"), "should name what ran: {err}");
}

// ---- the observation-only error report ---------------------------------------------------------

#[test]
fn observations_report_facts_and_never_a_guessed_cause() {
    let obs = transport().observations();
    assert!(obs.contains("/usr/local/bin/claude"), "{obs}");
    assert!(obs.contains("2.1.219"), "{obs}");
    assert!(
        obs.contains(MIN_CLAUDE_VERSION),
        "should name the supported floor: {obs}"
    );
    // `which` proved only that a file of this name exists, so a cause would be a guess.
    assert!(
        !obs.to_lowercase().contains("logged out"),
        "must not guess a cause: {obs}"
    );
    assert!(
        !obs.to_lowercase().contains("not logged in"),
        "must not guess a cause: {obs}"
    );
}

#[test]
fn exit_failure_reports_code_stderr_observations_and_the_escape_hatch() {
    let output = std::process::Output {
        status: exit_status(1),
        stdout: Vec::new(),
        stderr: b"Invalid API key - please run /login".to_vec(),
    };
    let msg = transport().exit_failure(&output);
    assert!(msg.contains("exit 1"), "{msg}");
    assert!(
        msg.contains("Invalid API key"),
        "must surface the child's stderr verbatim: {msg}"
    );
    assert!(msg.contains("/usr/local/bin/claude"), "{msg}");
    assert!(
        msg.contains("check the install and login"),
        "must carry the escape hatch: {msg}"
    );
}

#[test]
fn exit_failure_enriches_with_an_envelope_message_when_one_exists() {
    // If the CLI managed to say what went wrong, that sentence beats the exit code.
    let output = std::process::Output {
        status: exit_status(1),
        stdout: br#"{"is_error":true,"error":{"message":"OAuth token has expired"}}"#.to_vec(),
        stderr: Vec::new(),
    };
    let msg = transport().exit_failure(&output);
    assert!(msg.contains("OAuth token has expired"), "got: {msg}");
}

#[test]
fn exit_failure_handles_empty_stderr_without_an_awkward_blank() {
    let output = std::process::Output {
        status: exit_status(2),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let msg = transport().exit_failure(&output);
    assert!(msg.contains("<empty>"), "got: {msg}");
}

#[test]
fn preview_truncates_by_chars_and_survives_multibyte() {
    // Byte-slicing a multibyte boundary would panic; this must not.
    let long = "é".repeat(STDERR_PREVIEW_BYTES * 2);
    let out = preview(long.as_bytes());
    assert_eq!(out.chars().count(), STDERR_PREVIEW_BYTES);
}
