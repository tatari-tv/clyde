#![allow(clippy::unwrap_used)]

use super::*;
use common::config::{DEFAULT_JUDGE_MAX_OUTPUT_TOKENS, DEFAULT_MODEL as MODEL, DEFAULT_SLOT_MAX_OUTPUT_TOKENS};

use crate::ENV_LOCK;
use crate::summarize::Kind;

/// The prompt/facts pair every body test uses. Small on purpose: the assertion is about the
/// envelope's exact shape, not about payload size.
const PROMPT: &str = "Write the report.";
const JSON_BODY: &str = "{\"a\":1}";

/// A stand-in system prompt. `build_body` takes it as an argument, so its CONTENT is not this file's
/// concern -- the assertion is that it is placed in the `system` field verbatim.
const SYSTEM: &str = "You are a precise technical writer.";

/// A ceiling for the tests where the number is INERT -- they assert stream omission, prompt joining, or
/// model presence, and any value would do. Deliberately not either default: tracking a real default by
/// coincidence would drag these tests along the next time one moves.
const INERT_CEILING: u32 = 1_024;

/// The joined user message the api transport must produce: instruction, blank line, fenced facts.
fn expected_user_msg() -> String {
    format!("{PROMPT}\n\n```json\n{JSON_BODY}\n```\n")
}

// ---- AC3: the serialized request body must not rot ---------------------------------------------

/// A `Job` at its DEFAULT pins, which is what the byte-identical baseline is a baseline of.
fn default_job(kind: Kind) -> Job<'static> {
    let (model, max_output_tokens) = match kind {
        // A slot rides the markdown MODEL pin with its own, much smaller ceiling.
        Kind::Slot => (MODEL, DEFAULT_SLOT_MAX_OUTPUT_TOKENS),
        // The judge rides the markdown pins by design (`Kind::max_output_tokens_key`).
        Kind::Judge => (MODEL, DEFAULT_JUDGE_MAX_OUTPUT_TOKENS),
    };
    Job {
        kind,
        model,
        max_output_tokens,
    }
}

/// The slot body, byte for byte.
///
/// `stream` is gone from the struct entirely rather than serialized as `false`, so the wire bytes are
/// the SAME bytes the pre-inversion non-streaming path sent. Field order is declaration order, and
/// `max_tokens` is the current declared slot default. Any unintended drift in field order, omission,
/// or the default still fails here.
#[test]
fn slot_body_is_byte_identical_to_baseline() {
    let job = default_job(Kind::Slot);
    let body = build_body(job.model, SYSTEM, job.max_output_tokens, PROMPT, JSON_BODY);
    let expected = format!(
        r#"{{"model":"claude-opus-4-8","max_tokens":1500,"system":{},"messages":[{{"role":"user","content":{}}}]}}"#,
        serde_json::to_string(SYSTEM).unwrap(),
        serde_json::to_string(&expected_user_msg()).unwrap(),
    );
    assert_eq!(serde_json::to_string(&body).unwrap(), expected);
}

/// The judge body, which differs from a slot's only in its ceiling.
#[test]
fn judge_body_is_byte_identical_to_baseline() {
    let job = default_job(Kind::Judge);
    let body = build_body(job.model, SYSTEM, job.max_output_tokens, PROMPT, JSON_BODY);
    let expected = format!(
        r#"{{"model":"claude-opus-4-8","max_tokens":32000,"system":{},"messages":[{{"role":"user","content":{}}}]}}"#,
        serde_json::to_string(SYSTEM).unwrap(),
        serde_json::to_string(&expected_user_msg()).unwrap(),
    );
    assert_eq!(serde_json::to_string(&body).unwrap(), expected);
}

/// `stream` must not appear on the wire at all. Streaming existed only for the long html generation;
/// removing the field keeps the request bytes identical to what the non-streaming path always sent.
#[test]
fn the_request_body_carries_no_stream_field() {
    let job = default_job(Kind::Slot);
    let body = build_body(job.model, SYSTEM, job.max_output_tokens, PROMPT, JSON_BODY);
    assert!(!serde_json::to_string(&body).unwrap().contains("stream"));
}

// ---- the two api knobs, asserted SEPARATELY ----------------------------------------------------
//
// The retired `Job::api_limits()` returned them as one tuple, which packed the SHARED output ceiling
// and the api-PRIVATE streaming choice into a single value. Two signals never share one value here, so
// they get one assertion each.

#[test]
fn the_default_ceilings_are_the_documented_pair() {
    // A silent change to either ceiling fails here. The slot ceiling is orders of magnitude below the
    // whole-document one it replaced, and that gap IS the cost argument for the inversion.
    assert_eq!(DEFAULT_JUDGE_MAX_OUTPUT_TOKENS, 32_000);
    assert_eq!(DEFAULT_SLOT_MAX_OUTPUT_TOKENS, 1_500);
    const { assert!(DEFAULT_SLOT_MAX_OUTPUT_TOKENS < DEFAULT_JUDGE_MAX_OUTPUT_TOKENS) };
}

#[test]
fn the_model_default_is_opus_4_8() {
    // Scott, 2026-07-24: "just use claude opus 4-8".
    assert_eq!(MODEL, "claude-opus-4-8");
}

// ---- key resolution ---------------------------------------------------------------------------

#[test]
fn api_key_from_env_returns_none_when_unset() {
    let guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: serialized behind ENV_LOCK, and restored before the guard drops.
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    assert_eq!(api_key_from_env(), None);
    if let Some(v) = prev {
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }
    drop(guard);
}

#[test]
fn api_key_from_env_treats_whitespace_only_as_absent() {
    let guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: serialized behind ENV_LOCK, and restored before the guard drops.
    unsafe {
        std::env::set_var("ANTHROPIC_API_KEY", "   \t ");
    }
    // A whitespace-only key would otherwise sail past a bare is_empty check and fail at the API
    // with a 401 instead of naming the real problem locally.
    assert_eq!(api_key_from_env(), None);
    match prev {
        Some(v) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) },
        None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
    }
    drop(guard);
}

#[test]
fn from_env_error_names_both_remedies() {
    let guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: serialized behind ENV_LOCK, and restored before the guard drops.
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    let err = ApiTransport::from_env().unwrap_err().to_string();
    if let Some(v) = prev {
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }
    drop(guard);
    // There are two doors now, so an unset key must not read as "you need a key, full stop".
    assert!(err.contains("ANTHROPIC_API_KEY"), "should name the env var: {err}");
    assert!(err.contains("claude"), "should name the cli alternative: {err}");
}

/// AC11's api half: a CONFIGURED model must reach the wire, not just the default.
///
/// Every other body test passes `MODEL`, which EQUALS the literal
/// `"claude-opus-4-8"` those fixtures assert — so hardcoding the model inside `build_body` would
/// leave them all green. A sentinel that could never be a default closes that hole.
///
/// BITES: replace `model` with a literal in `build_body` and this fails.
#[test]
fn a_configured_model_reaches_the_serialized_body() {
    let body = build_body("sentinel-model-xyz", "sys", INERT_CEILING, PROMPT, JSON_BODY);
    let json = serde_json::to_string(&body).unwrap();
    assert!(
        json.contains(r#""model":"sentinel-model-xyz""#),
        "the configured pin must reach the wire: {json}"
    );
    assert!(!json.contains("claude-opus-4-8"), "no default may sneak in: {json}");
}

/// AC-C1's api half: a CONFIGURED ceiling must reach the wire as `max_tokens`.
///
/// The sentinel could never be a default, so a hardcoded ceiling inside `build_body` (or a `Job` built
/// from a const rather than from config) fails here. The cli half of the same probe lives in
/// `cli/tests.rs`, and together they cross both transports — which the byte-identical body tests never
/// do, since they call `build_body` directly and never touch a transport at all.
///
/// BITES: replace `max_tokens` with a literal in `build_body` and this fails.
#[test]
fn a_configured_ceiling_reaches_the_serialized_body() {
    let job = Job {
        kind: Kind::Slot,
        model: MODEL,
        max_output_tokens: 12_345,
    };
    let body = build_body(job.model, "sys", job.max_output_tokens, PROMPT, JSON_BODY);
    let json = serde_json::to_string(&body).unwrap();
    assert!(
        json.contains(r#""max_tokens":12345"#),
        "the configured ceiling must reach the wire: {json}"
    );
}
