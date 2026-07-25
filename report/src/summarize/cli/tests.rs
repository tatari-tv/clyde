#![allow(clippy::unwrap_used)]

use super::super::Kind;
use super::*;
use common::config::{DEFAULT_HTML_MAX_OUTPUT_TOKENS, DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS};

const MODEL: &str = "claude-opus-4-8";

/// The observation block a real transport would render, for guards that embed it.
const OBS: &str = "  binary:  /usr/local/bin/claude\n  version: 2.1.219 (minimum supported: 2.1.219)";

use crate::ENV_LOCK;

fn transport() -> CliTransport {
    CliTransport {
        binary: PathBuf::from("/usr/local/bin/claude"),
        version: "2.1.219 (Claude Code)".into(),
    }
}

/// A `Job` at its DEFAULT pins for the given kind. The one place these tests source a ceiling, so a
/// change to either default flows through every Guard 6 case instead of being hand-edited per site.
fn job(kind: Kind) -> Job<'static> {
    let max_output_tokens = match kind {
        Kind::Markdown => DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS,
        Kind::Html => DEFAULT_HTML_MAX_OUTPUT_TOKENS,
    };
    Job {
        kind,
        model: MODEL,
        max_output_tokens,
    }
}

/// A minimal successful envelope, parameterized so each guard test can spoil exactly one field.
fn envelope_json(
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
fn real_model_usage() -> String {
    r#"{"claude-haiku-4-5-20251001":{"canonicalModel":"claude-haiku-4-5","maxOutputTokens":32000},
       "claude-opus-4-8":{"canonicalModel":"claude-opus-4-8","maxOutputTokens":64000}}"#
        .into()
}

fn good_envelope() -> String {
    envelope_json(
        false,
        "success",
        "end_turn",
        "# Report\n\nprose",
        12_706,
        &real_model_usage(),
    )
}

// ---- AC8: the argv must carry every isolation flag --------------------------------------------

#[test]
fn argv_carries_every_isolation_flag_by_name() {
    let spawn = transport().build_spawn(job(Kind::Markdown), "SYS", "INSTRUCTION");
    let args = &spawn.args;
    let pos = |needle: &str| args.iter().position(|a| a == needle);

    // Each asserted BY NAME so none can be dropped silently in a later refactor.
    assert!(pos("--safe-mode").is_some(), "missing --safe-mode: {args:?}");
    assert!(
        pos("--strict-mcp-config").is_some(),
        "missing --strict-mcp-config: {args:?}"
    );
    assert!(
        pos("--no-session-persistence").is_some(),
        "missing --no-session-persistence: {args:?}"
    );
    // `--tools ""` is the structural tool-kill: the flag followed by an EMPTY string.
    let tools = pos("--tools").expect("missing --tools");
    assert_eq!(
        args[tools + 1],
        "",
        "--tools must be followed by an empty string: {args:?}"
    );
    // One turn.
    let turns = pos("--max-turns").expect("missing --max-turns");
    assert_eq!(args[turns + 1], "1");
    // JSON envelope.
    let fmt = pos("--output-format").expect("missing --output-format");
    assert_eq!(args[fmt + 1], "json");
}

#[test]
fn argv_never_passes_a_fallback_model() {
    let spawn = transport().build_spawn(job(Kind::Html), "SYS", "INSTRUCTION");
    // A fallback model would let the CLI silently swap models, defeating the canonicalModel guard.
    assert!(
        !spawn.args.iter().any(|a| a.contains("fallback")),
        "no --fallback-model may be passed: {:?}",
        spawn.args
    );
}

#[test]
fn argv_carries_the_configured_model_and_the_shared_system_prompt() {
    let spawn = transport().build_spawn(
        Job {
            kind: Kind::Markdown,
            model: "some-configured-model",
            max_output_tokens: DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS,
        },
        "THE-SYSTEM",
        "THE-INSTRUCTION",
    );
    let args = &spawn.args;
    let after = |needle: &str| {
        let i = args.iter().position(|a| a == needle).expect(needle);
        args[i + 1].clone()
    };
    // The config-resolved pin must reach --model (AC11's cli half).
    assert_eq!(after("--model"), "some-configured-model");
    assert_eq!(after("--system-prompt"), "THE-SYSTEM");
    // The instruction rides argv; the facts ride stdin.
    assert_eq!(after("-p"), "THE-INSTRUCTION");
    assert_eq!(spawn.program, PathBuf::from("/usr/local/bin/claude"));
}

// ---- AC4: the child inherits NOTHING ----------------------------------------------------------

#[test]
fn child_env_is_an_allowlist_and_leaks_no_secret() {
    // Holds ENV_LOCK even though it mutates nothing: `child_env()` READS the environment, and reading
    // the environ block while another test is inside `set_var` is the unsafety window edition 2024
    // made explicit. Every env-touching test takes this lock, readers included.
    let guard = ENV_LOCK.lock().unwrap();
    let env = child_env();
    drop(guard);
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

    // Enumerated BY NAME so a future secret-bearing variable fails loudly rather than leaking.
    for forbidden in [
        "ANTHROPIC_API_KEY",
        "CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY",
        "CLAUDE_COST_SLACK_APP_TOKEN",
        "CLAUDE_COST_SLACK_BOT_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_TMPDIR",
        "CLAUDE_EFFORT",
    ] {
        assert!(
            !names.contains(&forbidden),
            "{forbidden} must not reach the child: {names:?}"
        );
    }
    // And nothing CLAUDE*-shaped at all, so the next such variable is excluded by construction.
    assert!(
        !names.iter().any(|n| n.starts_with("CLAUDE")),
        "no CLAUDE* variable may reach the child: {names:?}"
    );
    // The allowlist is exactly the three documented entries (HOME only when resolvable).
    for name in &names {
        assert!(
            matches!(*name, "HOME" | "PATH" | "NO_UPDATE_NOTIFIER"),
            "unexpected variable in the allowlist: {name}"
        );
    }
    assert_eq!(
        env.iter()
            .find(|(k, _)| k == "NO_UPDATE_NOTIFIER")
            .map(|(_, v)| v.as_str()),
        Some("1"),
        "the update-notice guard must be set"
    );
}

#[test]
fn child_env_survives_a_secret_being_present_in_the_parent() {
    // The parent's env is irrelevant by construction (env_clear + allowlist), so setting a secret
    // here must change nothing. This is the property a denylist could not guarantee.
    let guard = ENV_LOCK.lock().unwrap();
    let before = child_env();
    // SAFETY: serialized behind ENV_LOCK; removed before the guard drops.
    unsafe {
        std::env::set_var("CLAUDE_COST_SLACK_BOT_TOKEN", "xoxb-not-a-real-token");
    }
    let after = child_env();
    unsafe {
        std::env::remove_var("CLAUDE_COST_SLACK_BOT_TOKEN");
    }
    drop(guard);
    assert_eq!(before, after, "the child env must not depend on the parent's");
}

/// AC4 clause one, proven by inspecting a REAL child's environment.
///
/// This spawns `/usr/bin/env` in place of `claude` and reads what the child actually received. An
/// earlier version of this test asserted `Command::get_envs().len()`, which does NOT work: that
/// getter reports only the explicit OVERRIDES, so deleting `cmd.env_clear()` left the assertion
/// passing while the child silently inherited the parent's entire environment — including the three
/// measured secrets below. The test was green and the security property was gone.
///
/// Nothing about this needs the `claude` binary, so the scope boundary ("no test shells out to the
/// real claude") is respected: `/usr/bin/env` is hermetic, fast, and present everywhere this builds.
///
/// BITES: delete `cmd.env_clear()` in `Spawn::to_command` and this fails on the planted secret.
#[test]
fn built_command_gives_the_child_only_the_allowlist_and_no_inherited_secret() {
    let guard = ENV_LOCK.lock().unwrap();
    // Plant a secret of each shape the design measured as leaking, in the PARENT.
    // Values are long and distinctive on purpose: the value-leak assertion below is a substring
    // search over the child's whole environment, so a short value like "1" would false-positive
    // against a legitimate allowlist entry (`NO_UPDATE_NOTIFIER=1`).
    let planted = [
        ("CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY", "planted-admin-key-must-not-leak"),
        ("CLAUDE_COST_SLACK_BOT_TOKEN", "planted-slack-bot-must-not-leak"),
        ("ANTHROPIC_API_KEY", "planted-api-key-must-not-leak"),
        ("CLAUDECODE", "planted-claudecode-must-not-leak"),
    ];
    // SAFETY: serialized behind ENV_LOCK; every planted var is removed below.
    for (k, v) in planted {
        unsafe { std::env::set_var(k, v) };
    }

    let mut spawn = transport().build_spawn(job(Kind::Markdown), "SYS", "P");
    // Swap the program for `env`, which prints the environment it was handed. The env/args split is
    // exactly what a real render builds; only the executable differs.
    spawn.program = PathBuf::from("/usr/bin/env");
    spawn.args.clear();
    let output = spawn.to_command().output().expect("/usr/bin/env must be spawnable");

    for (k, _) in planted {
        unsafe { std::env::remove_var(k) };
    }
    // `child_env()` READS the environment (`dirs::home_dir()`, `PATH`), so it must be called while
    // the lock is still held. Reading the environ block concurrently with another test's `set_var` is
    // the same unsafety window that makes `set_var` itself unsafe in edition 2024 — it can tear or
    // crash rather than fail cleanly. The assertion below cannot go WRONG (the allowlist can never
    // contain a planted secret), so this is purely about not reading a block mid-mutation.
    let allowlist = child_env();
    drop(guard);

    assert!(output.status.success(), "env exited {:?}", output.status.code());
    let seen: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let names: Vec<&str> = seen.iter().filter_map(|l| l.split('=').next()).collect();

    // Not one planted secret may appear, by name OR by value.
    for (k, v) in planted {
        assert!(!names.contains(&k), "{k} leaked into the child: {names:?}");
        assert!(
            !seen.iter().any(|l| l.contains(v)),
            "{k}'s VALUE leaked into the child under another name: {names:?}"
        );
    }
    // And the child's whole environment is the allowlist, nothing more. Both sides of this move
    // together if the allowlist changes, which is why the sibling
    // `child_env_is_an_allowlist_and_leaks_no_secret` pins the allowlist to its literal three names —
    // keep the pair together if either is ever refactored.
    let mut got = names.clone();
    got.sort_unstable();
    let mut want: Vec<&str> = allowlist.iter().map(|(k, _)| k.as_str()).collect();
    want.sort_unstable();
    assert_eq!(got, want, "child env must be exactly the allowlist");
}

// ---- envelope parsing: the two stdout-contamination guards -------------------------------------

#[test]
fn parse_envelope_reads_a_clean_envelope() {
    let env = parse_envelope(good_envelope().as_bytes()).unwrap();
    assert!(!env.is_error);
    assert_eq!(env.subtype.as_deref(), Some("success"));
    assert_eq!(env.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(env.usage.unwrap().output_tokens, Some(12_706));
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
    assert!(err.contains("--llm api"), "must carry the escape hatch: {err}");
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
    assert!(msg.contains("--llm api"), "must carry the escape hatch: {msg}");
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

/// Build a real `ExitStatus` with a given code, without spawning the thing under test.
fn exit_status(code: i32) -> std::process::ExitStatus {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exit {code}"))
        .status()
        .unwrap()
}

// ---- AC5: every guard in the chain bails loudly ------------------------------------------------
//
// Driven by recorded-shape envelope fixtures through the pure `check_envelope`, so no test shells out
// to the real `claude` binary. Each negative case was proven to bite by deleting its guard and
// watching the test fail.

fn check(json: &str, kind: Kind) -> Result<String> {
    check_envelope(parse_envelope(json.as_bytes()).unwrap(), job(kind), OBS)
}

#[test]
fn guards_pass_a_real_successful_envelope() {
    let out = check(&good_envelope(), Kind::Markdown).unwrap();
    assert_eq!(out, "# Report\n\nprose");
}

#[test]
fn guard_is_error_forwards_the_clis_own_message_verbatim() {
    // An expired token yields a WELL-FORMED envelope saying exactly what is wrong. Throwing that
    // sentence away in favor of a generic failure is the bug this guard exists to prevent.
    let json = r#"{"is_error":true,"subtype":"error_during_execution",
                   "error":{"message":"OAuth token has expired. Please run /login."}}"#;
    let err = check(json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("OAuth token has expired"), "verbatim message: {err}");
    assert!(err.contains("--llm api"), "escape hatch: {err}");
}

#[test]
fn guard_is_error_still_reports_when_the_envelope_carries_no_message() {
    let err = check(r#"{"is_error":true,"subtype":"error"}"#, Kind::Markdown)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no error message in the envelope"), "got: {err}");
}

#[test]
fn guard_subtype_bails_on_anything_but_success() {
    let json = envelope_json(false, "error_max_turns", "end_turn", "partial", 10, &real_model_usage());
    let err = check(&json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("subtype=error_max_turns"), "got: {err}");
    assert!(err.contains("expected \"success\""), "got: {err}");
}

#[test]
fn guard_subtype_bails_when_missing_entirely() {
    let json = r#"{"is_error":false,"stop_reason":"end_turn","result":"x"}"#;
    let err = check(json, Kind::Markdown).unwrap_err().to_string();
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
    let err = check(&json, Kind::Html).unwrap_err().to_string();
    assert!(err.contains("stop_reason=max_tokens"), "got: {err}");
    assert!(err.contains("truncated"), "must say the artifact is truncated: {err}");
    assert!(err.contains("--since"), "must name a remedy: {err}");
}

#[test]
fn guard_stop_reason_bails_when_missing() {
    let json = r#"{"is_error":false,"subtype":"success","result":"x"}"#;
    let err = check(json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("stop_reason=<missing>"), "got: {err}");
}

#[test]
fn guard_empty_result_bails_even_on_an_otherwise_clean_envelope() {
    // Exit 0, no error, end_turn, and nothing to show for it. Without this guard an empty artifact
    // would be published.
    let json = envelope_json(false, "success", "end_turn", "   \n  ", 0, &real_model_usage());
    let err = check(&json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("empty result"), "got: {err}");
}

#[test]
fn guard_output_ceiling_bails_when_the_job_budget_is_exceeded() {
    // `end_turn` proves a natural stop, NOT that output stayed under a ceiling the cli cannot set.
    // 20,000 tokens is a natural stop that still blows the markdown job's 16,000 budget.
    let json = envelope_json(false, "success", "end_turn", "long prose", 20_000, &real_model_usage());
    let err = check(&json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("20000 output tokens"), "got: {err}");
    assert!(err.contains("16000-token ceiling"), "must name the job ceiling: {err}");
}

#[test]
fn guard_output_ceiling_allows_the_same_output_for_the_larger_job() {
    // The ceiling is per-JOB: 20,000 tokens is over budget for markdown (16K) and fine for html (64K).
    let json = envelope_json(false, "success", "end_turn", "long prose", 20_000, &real_model_usage());
    assert!(check(&json, Kind::Markdown).is_err());
    assert_eq!(check(&json, Kind::Html).unwrap(), "long prose");
}

/// AC-C1's cli half: the ceiling Guard 6 enforces is the one on the `Job`, i.e. the one config
/// resolved — not a compile-time constant.
///
/// The sentinel could never be a default, so a Guard 6 that reads a const instead of `job` fails here.
/// The api half (`a_configured_ceiling_reaches_the_serialized_body`) proves the same value goes on the
/// wire, and together the two cross both transports.
///
/// BITES: replace `job.max_output_tokens` in Guard 6 with `{ let _ = job; 16_000 }` and this fails.
#[test]
fn guard_output_ceiling_enforces_the_configured_value_not_a_constant() {
    let configured = Job {
        kind: Kind::Markdown,
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

#[test]
fn guard_output_ceiling_accepts_exactly_the_ceiling() {
    // Boundary: the guard bails ABOVE the ceiling, not at it.
    let json = envelope_json(false, "success", "end_turn", "prose", 16_000, &real_model_usage());
    assert!(check(&json, Kind::Markdown).is_ok());
}

#[test]
fn guard_model_mismatch_bails_on_a_substituted_model() {
    let usage = r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#;
    let json = envelope_json(false, "success", "end_turn", "prose", 100, usage);
    let err = check(&json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("substituted model"), "got: {err}");
}

#[test]
fn guard_model_mismatch_error_carries_the_observations() {
    let usage = r#"{"claude-opus-4-8":{"canonicalModel":"claude-sonnet-5"}}"#;
    let json = envelope_json(false, "success", "end_turn", "prose", 100, usage);
    // The observations ride along via wrap_err, so an operator sees which binary produced this.
    let err = format!("{:#}", check(&json, Kind::Markdown).unwrap_err());
    assert!(err.contains("/usr/local/bin/claude"), "got: {err}");
}

#[test]
fn guards_run_in_order_so_the_most_specific_cause_wins() {
    // An envelope that is bad in several ways at once must report the CLI's own error message, not a
    // downstream symptom like the missing stop_reason.
    let json = r#"{"is_error":true,"subtype":"error","error":{"message":"rate limit exceeded"},
                   "result":""}"#;
    let err = check(json, Kind::Markdown).unwrap_err().to_string();
    assert!(err.contains("rate limit exceeded"), "got: {err}");
    assert!(
        !err.contains("<missing>"),
        "must not report a downstream symptom: {err}"
    );
}

// ---- the ESCAPE_HATCH contract, which nothing asserted either way ------------------------------

/// Failures the api transport could actually resolve must name it.
#[test]
fn credential_and_model_failures_carry_the_escape_hatch() {
    let cases = [
        // is_error: an expired token — api key would work.
        r#"{"is_error":true,"error":{"message":"OAuth token has expired"}}"#.to_string(),
        // subtype not success.
        envelope_json(false, "error_during_execution", "end_turn", "x", 1, &real_model_usage()),
        // empty result.
        envelope_json(false, "success", "end_turn", "  ", 1, &real_model_usage()),
        // a substituted model — the api path puts the pin on the wire and would honor it.
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
        let err = check(&json, Kind::Markdown).unwrap_err().to_string();
        assert!(err.contains("--llm api"), "must offer the api transport: {err}");
    }
}

/// Output-ceiling failures must NOT name it: the api path enforces the same per-job ceiling, so
/// `--llm api` would send the reader to a path that fails identically. A remedy that cannot remedy is
/// worse than none. (Audit finding; the invariant was previously stated absolutely and honored
/// partially, with nothing asserting either side.)
#[test]
fn ceiling_failures_do_not_offer_a_transport_that_fails_the_same_way() {
    // Guard 4: truncation.
    let truncated = envelope_json(false, "success", "max_tokens", "trunc", 16_000, &real_model_usage());
    let err = check(&truncated, Kind::Markdown).unwrap_err().to_string();
    assert!(!err.contains("--llm api"), "api path truncates identically: {err}");
    assert!(err.contains("--since"), "must still offer a remedy that works: {err}");

    // Guard 6: over budget on a natural stop.
    let over = envelope_json(false, "success", "end_turn", "long", 20_000, &real_model_usage());
    let err = check(&over, Kind::Markdown).unwrap_err().to_string();
    assert!(!err.contains("--llm api"), "api path caps at the same ceiling: {err}");
    assert!(err.contains("16000-token ceiling"), "must name the budget: {err}");
}
