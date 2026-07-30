#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- AC5: every guard in the chain bails loudly ------------------------------------------------
//
// Driven by recorded-shape envelope fixtures through the pure `check_envelope`, so no test shells out
// to the real `claude` binary. Each negative case was proven to bite by deleting its guard and
// watching the test fail.

#[test]
fn guards_pass_a_real_successful_envelope() {
    let out = check(&good_envelope(), Kind::Slot).unwrap();
    assert_eq!(out, "# Report\n\nprose");
}

/// PROVENANCE: this fixture is HAND-AUTHORED, not measured, unlike its dated neighbour below. Nobody
/// has observed what `claude` 2.1.220 emits for a real OAuth expiry, and proving it would require
/// logging the operator out (design Phase 0 Finding 8, "honest limit").
///
/// So it pins ONE thing and nothing more: that Guard 2 forwards whatever sentence the envelope carried,
/// verbatim. The sweep-fatal classifier is deliberately NOT built on it -- that reads
/// `api_error_status` and `terminal_reason`, both measured, and this shape carries neither, so it
/// classifies per-session. Do not add an assertion here about sweep-fatality: it would encode a guess.
#[test]
fn guard_is_error_forwards_the_clis_own_message_verbatim() {
    // An expired token yields a WELL-FORMED envelope saying exactly what is wrong. Throwing that
    // sentence away in favor of a generic failure is the bug this guard exists to prevent.
    let json = r#"{"is_error":true,"subtype":"error_during_execution",
                   "error":{"message":"OAuth token has expired. Please run /login."}}"#;
    let err = check(json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("OAuth token has expired"), "verbatim message: {err}");
    assert!(err.contains("check the install and login"), "escape hatch: {err}");
}

#[test]
fn guard_is_error_still_reports_when_the_envelope_carries_no_message() {
    let err = check(r#"{"is_error":true,"subtype":"error"}"#, Kind::Slot)
        .unwrap_err()
        .to_string();
    assert!(err.contains(NO_DETAIL_IN_ENVELOPE), "got: {err}");
}

/// The measured failure envelope (2026-07-26, sandboxed render): `is_error` with NO `error` field,
/// the diagnosis in `result`, the classification in `terminal_reason`. Mining `error.message` alone
/// reported an empty stderr and discarded the sentence that answered the question.
///
/// BITES: revert `failure_detail` to `error.message` only and both assertions fail.
#[test]
fn guard_is_error_falls_back_to_result_and_terminal_reason_when_no_error_field() {
    let json = r#"{"type":"result","is_error":true,"subtype":"error_during_execution",
                   "terminal_reason":"api_error",
                   "result":"API Error: Unable to connect to API (ENOTIMP)"}"#;
    let err = check(json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("Unable to connect to API (ENOTIMP)"), "got: {err}");
    assert!(err.contains("terminal_reason: api_error"), "got: {err}");
}

/// The same envelope on the OTHER path: a non-zero exit, where `claude` writes nothing to stderr.
/// This is where the defect was first seen -- `stderr: <empty>` and no message at all.
#[test]
fn exit_failure_falls_back_to_result_and_terminal_reason_when_no_error_field() {
    let output = std::process::Output {
        status: exit_status(1),
        stdout: br#"{"type":"result","is_error":true,"terminal_reason":"api_error",
                    "result":"API Error: Unable to connect to API (ENOTIMP)"}"#
            .to_vec(),
        stderr: Vec::new(),
    };
    let msg = transport().exit_failure(&output);
    assert!(msg.contains("Unable to connect to API (ENOTIMP)"), "got: {msg}");
    assert!(msg.contains("terminal_reason: api_error"), "got: {msg}");
    assert!(
        msg.contains("<empty>"),
        "the empty stderr is still reported as observed: {msg}"
    );
}

/// `error.message` still wins when it exists, and the classification rides along with it.
#[test]
fn failure_detail_prefers_the_error_message_and_appends_the_terminal_reason() {
    let envelope: Envelope = serde_json::from_str(
        r#"{"is_error":true,"terminal_reason":"api_error","result":"ignored when a message exists",
            "error":{"message":"OAuth token has expired"}}"#,
    )
    .unwrap();
    let detail = failure_detail(&envelope).unwrap();
    assert_eq!(detail, "OAuth token has expired (terminal_reason: api_error)");
}

/// A `terminal_reason` on its own is still worth more than nothing.
#[test]
fn failure_detail_reports_a_bare_terminal_reason() {
    let envelope: Envelope = serde_json::from_str(r#"{"is_error":true,"terminal_reason":"api_error"}"#).unwrap();
    assert_eq!(failure_detail(&envelope).unwrap(), "terminal_reason: api_error");
}

/// An envelope with nothing to say returns `None`, so Guard 2 prints its named last resort rather
/// than an empty string.
#[test]
fn failure_detail_is_none_when_the_envelope_says_nothing() {
    let envelope: Envelope = serde_json::from_str(r#"{"is_error":true,"result":"   "}"#).unwrap();
    assert!(failure_detail(&envelope).is_none());
}

/// A half-failed call can carry a truncated ARTIFACT in `result`; the error report must not become
/// the artifact.
#[test]
fn failure_detail_bounds_a_long_result() {
    let long = "x".repeat(STDERR_PREVIEW_BYTES * 3);
    let envelope: Envelope = serde_json::from_str(&format!(r#"{{"is_error":true,"result":"{long}"}}"#)).unwrap();
    let detail = failure_detail(&envelope).unwrap();
    assert_eq!(detail.chars().count(), STDERR_PREVIEW_BYTES);
}

#[test]
fn guard_subtype_bails_on_anything_but_success() {
    let json = envelope_json(false, "error_max_turns", "end_turn", "partial", 10, &real_model_usage());
    let err = check(&json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("subtype=error_max_turns"), "got: {err}");
    assert!(err.contains("expected \"success\""), "got: {err}");
}

#[test]
fn guard_subtype_bails_when_missing_entirely() {
    let json = r#"{"is_error":false,"stop_reason":"end_turn","result":"x"}"#;
    let err = check(json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("subtype=<missing>"), "got: {err}");
}

#[test]
fn guard_stop_reason_bails_on_max_tokens_truncation() {
    // A truncated artifact must NEVER be written. This is the same contract the api path enforces via
    // check_stop_reason, held here for the transport that cannot set max_tokens.
    let json = envelope_json(
        false,
        "success",
        "max_tokens",
        "<!doctype html><html>trunc",
        64_000,
        &real_model_usage(),
    );
    let err = check(&json, Kind::Judge).unwrap_err().to_string();
    assert!(err.contains("stop_reason=max_tokens"), "got: {err}");
    assert!(err.contains("truncated"), "must say the artifact is truncated: {err}");
    assert!(err.contains("--since"), "must name a remedy: {err}");
}

#[test]
fn guard_stop_reason_bails_when_missing() {
    let json = r#"{"is_error":false,"subtype":"success","result":"x"}"#;
    let err = check(json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("stop_reason=<missing>"), "got: {err}");
}

#[test]
fn guard_empty_result_bails_even_on_an_otherwise_clean_envelope() {
    // Exit 0, no error, end_turn, and nothing to show for it. Without this guard an empty artifact
    // would be published.
    let json = envelope_json(false, "success", "end_turn", "   \n  ", 0, &real_model_usage());
    let err = check(&json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("empty result"), "got: {err}");
}

#[test]
fn guard_output_ceiling_bails_when_the_job_budget_is_exceeded() {
    // `end_turn` proves a natural stop, NOT that output stayed under a ceiling the cli cannot set.
    // 5,000 tokens is a natural stop that still blows a slot's budget.
    let json = envelope_json(false, "success", "end_turn", "long prose", 5_000, &real_model_usage());
    let err = check(&json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("5000 output tokens"), "got: {err}");
    assert!(err.contains("1500-token ceiling"), "must name the job ceiling: {err}");
    // A budget the user set deserves the line that raises it (Alternative 4: the tokens are already
    // billed by the time this fires, so an actionable error is all that is left to salvage).
    assert!(
        err.contains("render.slot-max-output-tokens"),
        "must name the config key that raises the budget: {err}"
    );
}

#[test]
fn guard_output_ceiling_allows_the_same_output_for_the_larger_job() {
    // The ceiling is per-JOB, and the value must sit STRICTLY BETWEEN the two ceilings or this test
    // passes while proving nothing. 5,000 is over budget for a slot (1.5K) and fine for the judge
    // (32K); a value under both, or over both, would be the exact vacuous shape AC-C6 exists to catch.
    let json = envelope_json(false, "success", "end_turn", "long prose", 5_000, &real_model_usage());
    assert!(check(&json, Kind::Slot).is_err());
    assert_eq!(check(&json, Kind::Judge).unwrap(), "long prose");
}

/// AC-C1's cli half: the ceiling Guard 6 enforces is the one on the `Job`, i.e. the one config
/// resolved -- not a compile-time constant.
///
/// The sentinel could never be a default, so a Guard 6 that reads a const instead of `job` fails here.
/// The api half (`a_configured_ceiling_reaches_the_serialized_body`) proves the same value goes on the
/// wire, and together the two cross both transports.
///
/// BITES: replace `job.max_output_tokens` in Guard 6 with any literal and this fails.
#[test]
fn guard_output_ceiling_enforces_the_configured_value_not_a_constant() {
    let configured = Job {
        kind: Kind::Slot,
        model: MODEL,
        max_output_tokens: 12_345,
    };
    let json = envelope_json(false, "success", "end_turn", "long prose", 12_346, &real_model_usage());
    let err = check_envelope(parse_envelope(json.as_bytes()).unwrap(), configured, OBS)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("12345-token ceiling"),
        "the CONFIGURED ceiling must be the one enforced and reported: {err}"
    );

    // And one token under it passes, so the sentinel is the actual boundary rather than a coincidence.
    let under = envelope_json(false, "success", "end_turn", "long prose", 12_345, &real_model_usage());
    check_envelope(parse_envelope(under.as_bytes()).unwrap(), configured, OBS)
        .expect("exactly the configured ceiling must pass");
}

/// The JUDGE arm of Guard 6's bail, which nothing else exercises.
///
/// `each_kind_names_its_own_ceiling_key` proves the key FUNCTION returns the right key per kind, and
/// the pass-path test drives `Kind::Judge` only down the PASS path. Neither proves Guard 6 composes
/// the judge's key into a real bail, so a `max_output_tokens_key()` that was correct in isolation
/// could still be wired wrong here and no test would notice. Audit finding F3.
///
/// BITES: hardcode `Kind::Slot.max_output_tokens_key()` into the Guard 6 bail and this fails while
/// every other ceiling test stays green.
#[test]
fn guard_output_ceiling_names_the_judge_key_when_the_judge_job_is_over() {
    let over = u64::from(DEFAULT_JUDGE_MAX_OUTPUT_TOKENS) + 1;
    let json = envelope_json(false, "success", "end_turn", "long prose", over, &real_model_usage());
    let err = check(&json, Kind::Judge).unwrap_err().to_string();
    assert!(
        err.contains("render.judge-max-output-tokens"),
        "the judge bail must name the key that governs it: {err}"
    );
    assert!(
        !err.contains("render.slot-max-output-tokens"),
        "naming the slot key on a judge failure is a remedy that cannot remedy: {err}"
    );
    assert!(
        err.contains("32000-token ceiling"),
        "must name the judge ceiling: {err}"
    );
}

#[test]
fn guard_output_ceiling_accepts_exactly_the_ceiling() {
    // Boundary: the guard bails ABOVE the ceiling, not at it. Must equal the markdown default, or it
    // stops being a boundary test.
    let json = envelope_json(
        false,
        "success",
        "end_turn",
        "prose",
        u64::from(DEFAULT_SLOT_MAX_OUTPUT_TOKENS),
        &real_model_usage(),
    );
    assert!(check(&json, Kind::Slot).is_ok());
}
