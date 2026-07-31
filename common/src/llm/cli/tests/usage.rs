#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- Phase 2: Usage decodes input tokens, summed across every bucket the CLI bills -------------
//
// Design `2026-07-29-excise-api-key.md` Phase 2, Data Model. The probe fixtures are Phase 0's REAL
// measured envelopes (implementation-notes.md Finding 7), not invented numbers.

#[test]
fn tokens_in_is_plain_input_tokens_when_no_cache_buckets_are_present() {
    // Shape: input-only.
    let json = envelope_with_usage(Some(r#"{"input_tokens":40,"output_tokens":5}"#));
    let usage = parse_envelope(json.as_bytes()).unwrap().usage.unwrap();
    assert_eq!(usage.tokens_in(), 40);
    assert_eq!(usage.tokens_out(), 5);
}

#[test]
fn tokens_in_is_cache_write_alone_when_plain_input_tokens_is_absent() {
    // Shape: cache-write-only.
    let json = envelope_with_usage(Some(r#"{"cache_creation_input_tokens":900,"output_tokens":12}"#));
    let usage = parse_envelope(json.as_bytes()).unwrap().usage.unwrap();
    assert_eq!(usage.tokens_in(), 900);
    assert_eq!(usage.tokens_out(), 12);
}

#[test]
fn tokens_in_sums_input_and_cache_write_probe_a() {
    // Shape: both. Probe A (12,320 B payload), Finding 7: tokens_in = 4346, tokens_out = 5798.
    // Carries `service_tier` and `cache_creation`, fields `Usage` does not name, proving
    // unknown-field tolerance inside `usage` survives the extension.
    let usage_json = r#"{"input_tokens":10,"cache_creation_input_tokens":4336,
        "cache_read_input_tokens":0,"output_tokens":5798,"service_tier":"standard",
        "cache_creation":{"ephemeral_1h_input_tokens":4336,"ephemeral_5m_input_tokens":0}}"#;
    let json = envelope_with_usage(Some(usage_json));
    let usage = parse_envelope(json.as_bytes()).unwrap().usage.unwrap();
    assert_eq!(
        usage.tokens_in(),
        4346,
        "10 input_tokens + 4336 cache-write + 0 cache-read"
    );
    assert_eq!(usage.tokens_out(), 5798);
}

#[test]
fn tokens_in_sums_input_and_cache_write_when_cache_read_is_absent_probe_b() {
    // Shape: both, minus one bucket. Probe B (125,267 B payload), Finding 7: tokens_in = 35813,
    // tokens_out = 678. `cache_read_input_tokens` is ABSENT from the real envelope -- exactly the
    // shape `#[serde(default)]` must carry as zero rather than fail to parse.
    let usage_json = r#"{"input_tokens":10,"cache_creation_input_tokens":35803,"output_tokens":678}"#;
    let json = envelope_with_usage(Some(usage_json));
    let usage = parse_envelope(json.as_bytes()).unwrap().usage.unwrap();
    assert_eq!(
        usage.tokens_in(),
        35813,
        "10 input_tokens + 35803 cache-write, cache_read absent"
    );
    assert_eq!(usage.tokens_out(), 678);
}

/// Shape: absent. Absent `usage` on an otherwise-successful envelope is a hard error, never a
/// silently-recorded zero (design Data Model). BITES: soften the Guard 6 bail to a default `Usage`
/// and this fails -- see the break-it-to-prove-it note in the Phase 2 implementation notes.
#[test]
fn check_envelope_bails_when_usage_is_absent_from_a_success_envelope_and_names_the_job() {
    let json = envelope_with_usage(None);
    let err = check(&json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("Slot"), "must name the job: {err}");
    assert!(err.contains("no usage"), "got: {err}");
}

/// The job-naming half of the same guard, for the other kind, so a bail that only ever names one
/// kind by accident (a hardcoded `Kind::Slot` in the message) cannot pass silently.
#[test]
fn check_envelope_bails_naming_the_judge_when_usage_is_absent() {
    let json = envelope_with_usage(None);
    let err = check(&json, Kind::Judge).unwrap_err().to_string();
    assert!(err.contains("Judge"), "must name the job: {err}");
}

#[test]
fn guard_model_mismatch_bails_on_a_substituted_model() {
    let usage = r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#;
    let json = envelope_json(false, "success", "end_turn", "prose", 100, usage);
    let err = check(&json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("substituted model"), "got: {err}");
}

#[test]
fn guard_model_mismatch_error_carries_the_observations() {
    let usage = r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#;
    let json = envelope_json(false, "success", "end_turn", "prose", 100, usage);
    // The observations ride along via wrap_err, so an operator sees which binary produced this.
    let err = format!("{:#}", check(&json, Kind::Slot).unwrap_err());
    assert!(err.contains("/usr/local/bin/claude"), "got: {err}");
}

#[test]
fn guards_run_in_order_so_the_most_specific_cause_wins() {
    // An envelope that is bad in several ways at once must report the CLI's own error message, not a
    // downstream symptom like the missing stop_reason.
    let json = r#"{"is_error":true,"subtype":"error","error":{"message":"rate limit exceeded"},
                   "result":""}"#;
    let err = check(json, Kind::Slot).unwrap_err().to_string();
    assert!(err.contains("rate limit exceeded"), "got: {err}");
    assert!(
        !err.contains("<missing>"),
        "must not report a downstream symptom: {err}"
    );
}
