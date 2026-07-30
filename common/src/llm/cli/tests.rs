#![allow(clippy::unwrap_used)]

use super::super::Kind;
use super::*;
use crate::config::{DEFAULT_JUDGE_MAX_OUTPUT_TOKENS, DEFAULT_SLOT_MAX_OUTPUT_TOKENS};

pub(super) const MODEL: &str = "claude-opus-4-8";

/// The observation block a real transport would render, for guards that embed it.
pub(super) const OBS: &str = "  binary:  /usr/local/bin/claude\n  version: 2.1.219 (minimum supported: 2.1.219)";

use crate::ENV_LOCK;

pub(super) fn transport() -> CliTransport {
    CliTransport {
        binary: PathBuf::from("/usr/local/bin/claude"),
        version: "2.1.219 (Claude Code)".into(),
    }
}

/// The ceiling `sessions::llm` pins for enrich and narrate. A literal here because `common` cannot
/// depend on `sessions`; its only role in these tests is to prove the ceiling is NOT enforced for those
/// kinds, since their real output is measured in the thousands (Phase 0 Finding 3).
pub(super) const SESSIONS_MAX_OUTPUT_TOKENS: u32 = 512;

/// A `Job` at its DEFAULT pins for the given kind. The one place these tests source a ceiling, so a
/// change to either default flows through every Guard 6 case instead of being hand-edited per site.
pub(super) fn job(kind: Kind) -> Job<'static> {
    let max_output_tokens = match kind {
        Kind::Slot => DEFAULT_SLOT_MAX_OUTPUT_TOKENS,
        // The judge rides the markdown pins by design (`Kind::max_output_tokens_key`).
        Kind::Judge => DEFAULT_JUDGE_MAX_OUTPUT_TOKENS,
        Kind::Enrich | Kind::Narrate => SESSIONS_MAX_OUTPUT_TOKENS,
    };
    Job {
        kind,
        model: MODEL,
        max_output_tokens,
    }
}

/// Every kind, so a test that must hold "for exactly one kind" enumerates rather than samples.
pub(super) const ALL_KINDS: [Kind; 4] = [Kind::Slot, Kind::Judge, Kind::Enrich, Kind::Narrate];

/// A minimal successful envelope, parameterized so each guard test can spoil exactly one field.
pub(super) fn envelope_json(
    is_error: bool,
    subtype: &str,
    stop_reason: &str,
    result: &str,
    output_tokens: u64,
    model_usage: &str,
) -> String {
    format!(
        r#"{{"type":"result","is_error":{is_error},"subtype":"{subtype}","stop_reason":"{stop_reason}",
        "result":{},"usage":{{"output_tokens":{output_tokens},"input_tokens":1}},
        "modelUsage":{model_usage},"session_id":"abc","duration_ms":1234,"total_cost_usd":2.93}}"#,
        serde_json::to_string(result).unwrap()
    )
}

/// The real shape measured on 2026-07-24: the pinned model AND an internal haiku sub-call.
pub(super) fn real_model_usage() -> String {
    r#"{"claude-haiku-4-5-20251001":{"canonicalModel":"claude-haiku-4-5","maxOutputTokens":32000},
       "claude-opus-4-8":{"canonicalModel":"claude-opus-4-8","maxOutputTokens":64000}}"#
        .into()
}

pub(super) fn good_envelope() -> String {
    envelope_json(
        false,
        "success",
        "end_turn",
        "# Report\n\nprose",
        // A slot-sized output: the Phase 0 live measurement of the `executive-summary` slot. The old
        // 12,706 was a whole-DOCUMENT render, which no longer fits under any live job's ceiling.
        217,
        &real_model_usage(),
    )
}

pub(super) fn check(json: &str, kind: Kind) -> Result<String> {
    check_full(json, kind).map(|c| c.text)
}

/// The same, keeping the token counts, for the cases that assert on them.
pub(super) fn check_full(json: &str, kind: Kind) -> Result<Completion> {
    check_envelope(parse_envelope(json.as_bytes()).unwrap(), job(kind), OBS)
}

/// Build a real `ExitStatus` with a given code, without spawning the thing under test.
pub(super) fn exit_status(code: i32) -> std::process::ExitStatus {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exit {code}"))
        .status()
        .unwrap()
}

/// A minimal, otherwise-successful envelope carrying exactly the given `usage` object, or none at
/// all (`usage_field: None`), so Guard 6 (usage presence) can be exercised directly.
pub(super) fn envelope_with_usage(usage_field: Option<&str>) -> String {
    match usage_field {
        Some(usage) => format!(
            r#"{{"is_error":false,"subtype":"success","stop_reason":"end_turn","result":"x",
                "usage":{usage},"modelUsage":{}}}"#,
            real_model_usage()
        ),
        None => format!(
            r#"{{"is_error":false,"subtype":"success","stop_reason":"end_turn","result":"x",
                "modelUsage":{}}}"#,
            real_model_usage()
        ),
    }
}

// The section banners of the former single-file version are now submodules, one per contiguous run.
// Declared here rather than in a `mod.rs`, keeping the repo's Rust 2018+ module style: `tests.rs` is
// the entry point and `tests/` holds its children. The helpers above are `pub(super)` because every
// use of them is now a cross-module one.
mod argv;
mod env;
mod envelope;
mod fatal;
mod guards;
mod kinds;
mod usage;
