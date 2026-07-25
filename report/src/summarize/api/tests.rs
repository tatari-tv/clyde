#![allow(clippy::unwrap_used)]

use super::super::{HTML_SYSTEM_PROMPT, MARKDOWN_SYSTEM_PROMPT};
use super::*;
use common::config::{
    DEFAULT_HTML_MAX_OUTPUT_TOKENS, DEFAULT_HTML_MODEL as HTML_MODEL, DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS,
    DEFAULT_MARKDOWN_MODEL as MARKDOWN_MODEL,
};

use crate::ENV_LOCK;

/// The prompt/facts pair every body test uses. Small on purpose: the assertion is about the
/// envelope's exact shape, not about payload size.
const PROMPT: &str = "Write the report.";
const JSON_BODY: &str = "{\"a\":1}";

/// The joined user message the api transport must produce: instruction, blank line, fenced facts.
fn expected_user_msg() -> String {
    format!("{PROMPT}\n\n```json\n{JSON_BODY}\n```\n")
}

// ---- AC3: the serialized request body must not rot ---------------------------------------------

/// A `Job` at its DEFAULT pins, which is what the byte-identical baseline is a baseline of.
fn default_job(kind: Kind) -> Job<'static> {
    let (model, max_output_tokens) = match kind {
        Kind::Markdown => (MARKDOWN_MODEL, DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS),
        Kind::Html => (HTML_MODEL, DEFAULT_HTML_MAX_OUTPUT_TOKENS),
    };
    Job {
        kind,
        model,
        max_output_tokens,
    }
}

#[test]
fn markdown_body_is_byte_identical_to_baseline() {
    let job = default_job(Kind::Markdown);
    let body = build_body(
        job.model,
        MARKDOWN_SYSTEM_PROMPT,
        job.max_output_tokens,
        job.kind.streams(),
        PROMPT,
        JSON_BODY,
    );
    // Field order is the struct's declaration order, `stream` is OMITTED entirely when false, and
    // `max_tokens` is the unchanged 16K markdown ceiling. Any drift in any of those fails here.
    let expected = format!(
        r#"{{"model":"claude-opus-4-8","max_tokens":16000,"system":{},"messages":[{{"role":"user","content":{}}}]}}"#,
        serde_json::to_string(MARKDOWN_SYSTEM_PROMPT).unwrap(),
        serde_json::to_string(&expected_user_msg()).unwrap(),
    );
    assert_eq!(serde_json::to_string(&body).unwrap(), expected);
}

#[test]
fn html_body_is_byte_identical_to_baseline() {
    let job = default_job(Kind::Html);
    let body = build_body(
        job.model,
        HTML_SYSTEM_PROMPT,
        job.max_output_tokens,
        job.kind.streams(),
        PROMPT,
        JSON_BODY,
    );
    // Same shape as markdown but `stream: true` IS serialized, and the ceiling is 64K.
    let expected = format!(
        r#"{{"model":"claude-opus-4-8","max_tokens":64000,"stream":true,"system":{},"messages":[{{"role":"user","content":{}}}]}}"#,
        serde_json::to_string(HTML_SYSTEM_PROMPT).unwrap(),
        serde_json::to_string(&expected_user_msg()).unwrap(),
    );
    assert_eq!(serde_json::to_string(&body).unwrap(), expected);
}

#[test]
fn stream_false_is_omitted_not_serialized_as_false() {
    let body = build_body(MARKDOWN_MODEL, "sys", 16_000, false, PROMPT, JSON_BODY);
    let json = serde_json::to_string(&body).unwrap();
    // A `"stream":false` on the wire would be a behavior change from the pre-HTML baseline even
    // though it is semantically equivalent. Assert the absence explicitly.
    assert!(
        !json.contains("stream"),
        "stream must be omitted when false, got: {json}"
    );
}

#[test]
fn body_joins_prompt_and_facts_with_a_fenced_json_block() {
    let body = build_body(
        MARKDOWN_MODEL,
        "sys",
        16_000,
        false,
        "  trailing space trimmed  ",
        JSON_BODY,
    );
    let content = &body.messages.first().unwrap().content;
    // The prompt is right-trimmed, then a blank line, then the fenced facts. The cli transport sends
    // this same fenced block on stdin, so the model reads identical content on both transports.
    assert_eq!(content, "  trailing space trimmed\n\n```json\n{\"a\":1}\n```\n");
}

// ---- the two api knobs, asserted SEPARATELY ----------------------------------------------------
//
// The retired `Job::api_limits()` returned them as one tuple, which packed the SHARED output ceiling
// and the api-PRIVATE streaming choice into a single value. Two signals never share one value here, so
// they get one assertion each.

#[test]
fn streaming_is_derived_from_the_kind_not_from_a_threshold() {
    // Markdown reads a single JSON body; html streams so the 300s idle wall never fires on a long
    // generation. Derived from the KIND, never from a threshold over max_tokens.
    assert!(!Kind::Markdown.streams());
    assert!(Kind::Html.streams());
}

#[test]
fn the_default_ceilings_are_the_documented_pair() {
    // Mirrors how `both_jobs_default_to_opus_4_8` pins the model defaults: a silent change to either
    // ceiling fails here.
    assert_eq!(DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS, 16_000);
    assert_eq!(DEFAULT_HTML_MAX_OUTPUT_TOKENS, 64_000);
}

#[test]
fn both_jobs_default_to_opus_4_8() {
    // Scott, 2026-07-24: "just use claude opus 4-8". The markdown job re-pinned off opus-4-7, so a
    // silent revert to a split pin fails here.
    assert_eq!(MARKDOWN_MODEL, "claude-opus-4-8");
    assert_eq!(HTML_MODEL, "claude-opus-4-8");
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
/// Every other body test passes `MARKDOWN_MODEL`/`HTML_MODEL`, which EQUAL the literal
/// `"claude-opus-4-8"` those fixtures assert — so hardcoding the model inside `build_body` would
/// leave them all green. A sentinel that could never be a default closes that hole.
///
/// BITES: replace `model` with a literal in `build_body` and this fails.
#[test]
fn a_configured_model_reaches_the_serialized_body() {
    let body = build_body("sentinel-model-xyz", "sys", 16_000, false, PROMPT, JSON_BODY);
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
        kind: Kind::Markdown,
        model: MARKDOWN_MODEL,
        max_output_tokens: 12_345,
    };
    let body = build_body(
        job.model,
        "sys",
        job.max_output_tokens,
        job.kind.streams(),
        PROMPT,
        JSON_BODY,
    );
    let json = serde_json::to_string(&body).unwrap();
    assert!(
        json.contains(r#""max_tokens":12345"#),
        "the configured ceiling must reach the wire: {json}"
    );
}
