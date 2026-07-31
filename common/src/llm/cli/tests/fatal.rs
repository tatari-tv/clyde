#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- the ESCAPE_HATCH contract, which nothing asserted either way ------------------------------

/// Failures that could plausibly be an install, login, credential, or model-selection problem must
/// carry the escape hatch.
#[test]
fn credential_and_model_failures_carry_the_escape_hatch() {
    let cases = [
        // is_error: an expired token -- checking the install and login would actually help here.
        r#"{"is_error":true,"error":{"message":"OAuth token has expired"}}"#.to_string(),
        // subtype not success.
        envelope_json(false, "error_during_execution", "end_turn", "x", 1, &real_model_usage()),
        // empty result.
        envelope_json(false, "success", "end_turn", "  ", 1, &real_model_usage()),
        // a substituted model -- a fresh login might select the right one.
        envelope_json(
            false,
            "success",
            "end_turn",
            "prose",
            10,
            r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#,
        ),
        // the pinned model never ran at all.
        envelope_json(
            false,
            "success",
            "end_turn",
            "prose",
            10,
            r#"{"claude-haiku-4-5-20251001":{"canonicalModel":"claude-haiku-4-5"}}"#,
        ),
    ];
    for json in cases {
        let err = check(&json, Kind::Slot).unwrap_err().to_string();
        assert!(
            err.contains("check the install and login"),
            "must offer the escape hatch: {err}"
        );
    }
}

/// Output-ceiling failures must NOT name it: a working install and login cannot set a wire-level
/// ceiling over this transport, so appending the escape hatch there would send the reader to a fix
/// that does not fix anything. A remedy that cannot remedy is worse than none. (Audit finding; the
/// invariant was previously stated absolutely and honored partially, with nothing asserting either
/// side.)
#[test]
fn ceiling_failures_do_not_offer_a_remedy_that_cannot_remedy() {
    // Guard 4: truncation.
    // The count is INERT here: Guard 4 fires on `stop_reason: max_tokens` before Guard 6 sees it.
    let truncated = envelope_json(false, "success", "max_tokens", "trunc", 1_024, &real_model_usage());
    let err = check(&truncated, Kind::Slot).unwrap_err().to_string();
    assert!(
        !err.contains("check the install and login"),
        "a working install and login cannot fix a truncation: {err}"
    );
    assert!(err.contains("--since"), "must still offer a remedy that works: {err}");

    // Guard 7: over budget on a natural stop.
    let over = envelope_json(false, "success", "end_turn", "long", 5_000, &real_model_usage());
    let err = check(&over, Kind::Slot).unwrap_err().to_string();
    assert!(
        !err.contains("check the install and login"),
        "a working install and login cannot raise a config ceiling: {err}"
    );
    assert!(err.contains("1500-token ceiling"), "must name the budget: {err}");
    // The remedy it DOES offer must be one that works: the config key, not the escape hatch.
    assert!(
        err.contains("render.slot-max-output-tokens"),
        "must offer the remedy that actually remedies: {err}"
    );
}

// ---- Phase 3 / G5: sweep-fatal classification, AT THE TRANSPORT LAYER ---------------------------
//
// This is where the bug would actually live. A `Fake` completer that RETURNS
// `TransportError::Unavailable` proves only that the sweep honors the variant; it cannot prove the
// transport ever produces it. Every fixture below is either a Phase 0 measurement or a shape the design's
// classification table names explicitly.

/// Whether the error a fixture produces carries the sweep-fatal variant. Drives every case below off the
/// real code path (`parse_envelope` -> `check_envelope`), so a parse failure is classified too.
fn is_unavailable(json: &str, kind: Kind) -> bool {
    let err = parse_envelope(json.as_bytes())
        .and_then(|e| check_envelope(e, job(kind), OBS))
        .expect_err("this fixture must fail");
    matches!(
        err.downcast_ref::<TransportError>(),
        Some(TransportError::Unavailable(_))
    )
}

/// The MEASURED auth failure (design Phase 0 Finding 8, probe C, verbatim): a rejected credential under
/// the exact argv this transport builds. `is_error: true` at `subtype: "success"` and exit 0, which is
/// why classification cannot read exit status, and `api_error_status: 401` is the typed discriminator.
///
/// BITES: drop `api_error_status` from `Envelope`, or drop the 401 arm from `is_sweep_fatal`, and this
/// fails -- which in production means an expired login charges a durable attempt to every candidate.
#[test]
fn the_measured_401_envelope_is_sweep_fatal() {
    let json = r#"{ "is_error": true, "subtype": "success", "stop_reason": "stop_sequence",
        "terminal_reason": "api_error", "api_error_status": 401,
        "result": "Invalid API key · Fix external API key",
        "total_cost_usd": 0, "duration_ms": 280, "modelUsage": {} }"#;
    assert!(is_unavailable(json, Kind::Enrich), "a 401 must abort the sweep");
    // And the operator still gets the CLI's own sentence, not a generic "unavailable".
    let err = check(json, Kind::Enrich).unwrap_err().to_string();
    assert!(err.contains("Invalid API key"), "verbatim detail: {err}");
    assert!(err.contains("unavailable"), "names the class: {err}");
}

/// The MEASURED network failure (Finding 9, probe D, and independently clyde's own dated 2026-07-26
/// fixture): `terminal_reason: "api_error"` with `api_error_status: null`. The belt-and-braces row.
///
/// BITES: delete the `None => terminal_reason == "api_error"` arm and this fails, and a dead transport
/// then charges an attempt per row for ~14 hours (179s per call, measured).
#[test]
fn the_measured_status_less_api_error_envelope_is_sweep_fatal() {
    let probe_d = r#"{ "is_error": true, "subtype": "success", "stop_reason": "stop_sequence",
        "terminal_reason": "api_error", "api_error_status": null,
        "result": "API Error: Unable to connect to API (ConnectionRefused)",
        "total_cost_usd": 0, "duration_ms": 176736 }"#;
    assert!(is_unavailable(probe_d, Kind::Enrich));

    // The dated fixture already in this file, four days earlier, different errno, same shape.
    let dated_2026_07_26 = r#"{"type":"result","is_error":true,"subtype":"error_during_execution",
        "terminal_reason":"api_error",
        "result":"API Error: Unable to connect to API (ENOTIMP)"}"#;
    assert!(is_unavailable(dated_2026_07_26, Kind::Enrich));
}

/// The rest of the table's sweep-fatal statuses, enumerated so none is classified by accident.
#[test]
fn rate_limit_forbidden_and_upstream_failures_are_sweep_fatal() {
    for status in [
        HTTP_UNAUTHORIZED,
        HTTP_FORBIDDEN,
        HTTP_TOO_MANY_REQUESTS,
        HTTP_SERVER_ERROR_FLOOR,
        502,
        HTTP_SERVER_ERROR_LIMIT - 1,
    ] {
        let json = format!(
            r#"{{"is_error":true,"subtype":"success","api_error_status":{status},"result":"upstream said no"}}"#
        );
        assert!(is_unavailable(&json, Kind::Enrich), "{status} must be sweep-fatal");
    }
}

/// The per-session side of the table. Each of these is a property of ONE call, so charging it one
/// durable attempt is the correct accounting -- and mis-classifying it as sweep-fatal would let three bad
/// head rows abort every sweep forever.
///
/// BITES: widen `is_sweep_fatal` to "any 4xx" or "any is_error" and the 400 case fails.
#[test]
fn per_session_failures_do_not_downcast_to_the_sweep_fatal_variant() {
    // A 400: the request we just sent was malformed. About this payload.
    let bad_request = r#"{"is_error":true,"subtype":"success","api_error_status":400,
        "result":"invalid request: messages.0 too long"}"#;
    assert!(!is_unavailable(bad_request, Kind::Enrich));

    // A malformed envelope: no JSON at all, and JSON that is not an envelope.
    assert!(!is_unavailable("not json at all", Kind::Enrich));
    assert!(!is_unavailable(r#"{"is_error": tru"#, Kind::Enrich));

    // A truncation: `stop_reason: max_tokens` on an otherwise-clean envelope.
    let truncated = envelope_json(false, "success", "max_tokens", "trunc", 10, &real_model_usage());
    assert!(!is_unavailable(&truncated, Kind::Slot));

    // An empty result, a model substitution, and an absent `usage`: all per-session.
    let empty = envelope_json(false, "success", "end_turn", "  ", 1, &real_model_usage());
    assert!(!is_unavailable(&empty, Kind::Slot));
    let substituted = envelope_json(
        false,
        "success",
        "end_turn",
        "prose",
        10,
        r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#,
    );
    assert!(!is_unavailable(&substituted, Kind::Slot));
    assert!(!is_unavailable(&envelope_with_usage(None), Kind::Slot));

    // An `is_error` envelope with no status and no `api_error` classification: the CLI did not call it a
    // transport failure, so neither do we.
    let unclassified = r#"{"is_error":true,"subtype":"error","error":{"message":"something went wrong"}}"#;
    assert!(!is_unavailable(unclassified, Kind::Slot));
}

/// A `claude` that cannot be spawned at all is sweep-fatal, and so is one that exits non-zero without an
/// envelope (Guard 1, the logged-out shape). Both drive the REAL `complete_with_usage` path; neither
/// needs the `claude` binary, same hygiene as the `/usr/bin/env` test above.
#[test]
fn a_binary_that_cannot_run_is_sweep_fatal() {
    let missing = CliTransport {
        binary: PathBuf::from("/nonexistent/claude"),
        version: "unknown".into(),
    };
    let err = missing
        .complete_with_usage(job(Kind::Enrich), "SYS", "", "facts")
        .expect_err("a missing binary cannot complete");
    assert!(
        matches!(
            err.downcast_ref::<TransportError>(),
            Some(TransportError::Unavailable(_))
        ),
        "a spawn failure is never about this payload: {err}"
    );

    // Exit non-zero, nothing on stdout: what a logged-out `claude` looks like.
    let failing = CliTransport {
        binary: PathBuf::from("/bin/false"),
        version: "unknown".into(),
    };
    let err = failing
        .complete_with_usage(job(Kind::Enrich), "SYS", "", "facts")
        .expect_err("a non-zero exit must fail");
    assert!(
        matches!(
            err.downcast_ref::<TransportError>(),
            Some(TransportError::Unavailable(_))
        ),
        "a non-zero exit with no envelope is the logged-out row: {err}"
    );
    assert!(
        err.to_string().contains("exit 1"),
        "still reports what it observed: {err}"
    );
}

/// Some failure shapes emit `errors` (plural) instead of `error`. Before this phase the array was
/// deserialized nowhere, so its message was silently dropped and the report fell back to
/// `terminal_reason` alone.
///
/// BITES: delete the `errors` field or its `or_else` arm in `failure_detail` and this fails.
#[test]
fn an_errors_array_is_read_when_there_is_no_singular_error() {
    let envelope: Envelope = serde_json::from_str(
        r#"{"is_error":true,"terminal_reason":"api_error",
            "errors":[{"message":"overloaded_error"},{"message":"second, ignored"}]}"#,
    )
    .unwrap();
    let detail = failure_detail(&envelope).unwrap();
    assert_eq!(detail, "overloaded_error (terminal_reason: api_error)");

    // The singular still wins when both are present, so the existing contract is unchanged.
    let both: Envelope = serde_json::from_str(
        r#"{"is_error":true,"error":{"message":"the singular one"},"errors":[{"message":"the plural one"}]}"#,
    )
    .unwrap();
    assert_eq!(failure_detail(&both).unwrap(), "the singular one");
}
