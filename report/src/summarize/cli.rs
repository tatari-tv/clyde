//! The keyless transport: shell out to the locally installed `claude` CLI in headless print mode.
//!
//! Every clyde user already has a logged-in Claude Code — that login is WHY they have sessions to
//! report on. This transport piggy-backs it, so clyde reads, stores, refreshes, and transmits NO
//! credential: the `claude` binary owns auth end to end. Same shape as the existing shell-outs to
//! `pandoc` and to `marquee` (which owns its own Okta tokens).
//!
//! Fail loud, never fall back. Once this transport is selected, EVERY failure is terminal — logged
//! out, non-zero exit, malformed envelope, non-`end_turn` stop, model mismatch, timeout. None of
//! them retry and none silently switch to the api transport, because a silent fallback would make
//! one command nondeterministic across two transports and two billing paths, and would hide a broken
//! login forever.
//!
//! Failures that could plausibly be fixed by the OTHER transport carry the [`ESCAPE_HATCH`]; the ones
//! that could not, deliberately do not. A truncation or an over-budget artifact is NOT one of them:
//! the api path enforces the identical per-job ceiling (it sets `max_tokens` on the wire and bails on
//! `stop_reason: max_tokens`), so advising `--llm api` there would send the reader to a path that
//! fails the same way. Suggesting a remedy that cannot work is worse than suggesting none.

use super::{Job, Transport};
use crate::proc;
use eyre::{Context, Result, bail};
use log::{debug, info};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// The binary we resolve off PATH. Mirrors `clyde::resolve_claude`.
const CLAUDE_BINARY: &str = "claude";

/// Minimum `claude` version this transport is verified against (2026-07-24 spike).
///
/// NOT enforced as a pre-flight version gate: the version string is a foreign format that could
/// change, and a brittle parse would fail closed on a CLI that actually works. Instead the resolved
/// version is logged on every render and named in every failure, so an unsupported-flag exit reads as
/// "your claude is older than the floor" instead of as a mystery. The floor exists because the argv
/// depends on `--tools`, `--safe-mode`, `--strict-mcp-config`, `--no-session-persistence`, and
/// `--max-turns` — and `--max-turns` is accepted but UNDOCUMENTED in 2.1.219, so it is exactly the
/// kind of flag that could vanish without a deprecation notice.
const MIN_CLAUDE_VERSION: &str = "2.1.219";

/// How much child stderr to quote in a failure. Enough to carry the real message, bounded so a
/// runaway stderr cannot become the error report.
const STDERR_PREVIEW_BYTES: usize = 500;

/// The keyless transport, holding the resolved binary and its reported version.
#[derive(Debug, Clone)]
pub struct CliTransport {
    binary: PathBuf,
    version: String,
}

impl CliTransport {
    /// Resolve `claude` on PATH and read its version.
    ///
    /// This is a PRESENCE check, never a success check (design decision, Scott 2026-07-24: "fail
    /// loud"). Resolving the binary proves only that an executable of that name exists; it
    /// distinguishes nothing about a stale version, a wrapper or shim, a broken install, bad global
    /// config, an expired login, a plan cap, or rate-limit exhaustion. Every one of those surfaces
    /// later as a terminal error that reports observations rather than a guessed cause.
    pub fn resolve() -> Result<Self> {
        debug!("CliTransport::resolve: looking for `{CLAUDE_BINARY}` on PATH");
        let binary = which::which(CLAUDE_BINARY).map_err(|e| {
            eyre::eyre!(
                "the `{CLAUDE_BINARY}` CLI was not found on PATH ({e}); install Claude Code and log in \
                 once, or pass --llm api to use ANTHROPIC_API_KEY"
            )
        })?;
        let version = probe_version(&binary);
        info!(
            "CliTransport::resolve: transport=cli binary={} version={}",
            binary.display(),
            version
        );
        Ok(Self { binary, version })
    }

    /// Build the child process spec. Pure, so the argv and the env can be asserted without spawning.
    fn build_spawn(&self, job: Job<'_>, system: &str, prompt: &str) -> Spawn {
        debug!(
            "CliTransport::build_spawn: job={job:?} system bytes={} prompt bytes={}",
            system.len(),
            prompt.len()
        );
        Spawn {
            program: self.binary.clone(),
            args: vec![
                // The instruction is small and fixed, so it rides argv; the ~500KB report facts ride
                // stdin, which has no ARG_MAX ceiling.
                "-p".into(),
                prompt.into(),
                "--model".into(),
                job.model.into(),
                "--output-format".into(),
                "json".into(),
                // The SAME system prompt the api path sends, so the model reads identical
                // instructions on both transports.
                "--system-prompt".into(),
                system.into(),
                // Disable ALL built-in tools, structurally. Help text: `Use "" to disable all
                // tools`. This deletes tool-list drift as a risk CLASS rather than mitigating it:
                // nothing is enumerated, so nothing can drift.
                "--tools".into(),
                String::new(),
                // No CLAUDE.md, skills, plugins, hooks, MCP, or agents; auth preserved. This is the
                // isolation mechanism. A temp cwd only ever defeated PROJECT CLAUDE.md discovery —
                // user and global customizations still loaded — so cwd is hygiene, not the control.
                "--safe-mode".into(),
                // No MCP servers from any config file.
                "--strict-mcp-config".into(),
                // Write nothing to disk. Verified 2026-07-24: session JSONL and lock-file counts are
                // unchanged across a render, so a render never becomes a session clyde catalogs.
                "--no-session-persistence".into(),
                // One turn. Accepted but undocumented in 2.1.219 (see MIN_CLAUDE_VERSION).
                "--max-turns".into(),
                "1".into(),
                // Deliberately NO --fallback-model, so the CLI cannot silently swap models.
            ],
            env: child_env(),
        }
    }
}

impl Transport for CliTransport {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String> {
        let spawn = self.build_spawn(job, system, prompt);
        // The IDENTICAL fenced block the api transport puts in its user message, so the model sees
        // the same content in the same order on both transports. Only the channel differs.
        let payload = format!("```json\n{json_body}\n```\n");
        info!(
            "CliTransport::complete: transport=cli job={job:?} binary={} version={} \
             payload bytes={}",
            self.binary.display(),
            self.version,
            payload.len()
        );

        let mut cmd = spawn.to_command();
        let binary = self.binary.clone();
        let output = proc::run_with_payload("claude -p", &mut cmd, &payload, move |e| {
            eyre::eyre!(
                "failed to invoke the `claude` CLI at {}: {e}; try `claude` interactively to check the \
                 install, or pass --llm api to use ANTHROPIC_API_KEY",
                binary.display()
            )
        })?;

        // GUARD 1: exit status, checked BEFORE parsing. A logged-out `claude` exits non-zero and
        // prints to stderr WITHOUT emitting a JSON envelope, so parsing first would report
        // "malformed envelope" for what is really "you are logged out".
        if !output.status.success() {
            bail!(self.exit_failure(&output));
        }

        let envelope = parse_envelope(&output.stdout)?;
        let result = check_envelope(envelope, job, &self.observations())?;
        debug!("CliTransport::complete: job={job:?} ok result bytes={}", result.len());
        Ok(result)
    }
}

/// Guards 2-7, applied to an already-parsed envelope from an already-successful exit.
///
/// Pure, and separate from [`CliTransport::complete`] so every failure mode is driven by a recorded
/// envelope fixture in tests rather than requiring a real `claude` subprocess. Returns the validated
/// artifact text. Every guard bails loudly; none of them degrade the artifact.
fn check_envelope(envelope: Envelope, job: Job<'_>, observations: &str) -> Result<String> {
    debug!(
        "check_envelope: job={job:?} is_error={} subtype={:?} stop_reason={:?}",
        envelope.is_error, envelope.subtype, envelope.stop_reason
    );

    // GUARD 2: the CLI's own error message, forwarded VERBATIM. An expired token produces a perfectly
    // well-formed envelope saying exactly what is wrong; reporting that as a generic failure throws
    // away the one useful sentence we were given.
    if envelope.is_error {
        let detail = failure_detail(&envelope).unwrap_or_else(|| NO_DETAIL_IN_ENVELOPE.to_string());
        bail!("claude -p reported an error: {detail}\n{observations}\n{ESCAPE_HATCH}");
    }

    // GUARD 3: subtype.
    match envelope.subtype.as_deref() {
        Some("success") => {}
        other => bail!(
            "claude -p returned subtype={} (expected \"success\")\n{observations}\n{ESCAPE_HATCH}",
            other.unwrap_or("<missing>"),
        ),
    }

    // GUARD 4: stop_reason. A non-`end_turn` stop means the artifact is truncated and must not be
    // published.
    match envelope.stop_reason.as_deref() {
        Some("end_turn") => {}
        other => bail!(
            "claude -p stopped with stop_reason={} (expected end_turn): the generated artifact was \
             truncated and will not be written. Narrow the window with a shorter --since, or use \
             --format markdown.\n{observations}",
            other.unwrap_or("<missing>"),
        ),
    }

    // GUARD 5: a non-empty result.
    let result = envelope.result.unwrap_or_default();
    if result.trim().is_empty() {
        bail!("claude -p returned an empty result with no error\n{observations}\n{ESCAPE_HATCH}");
    }

    // GUARD 6: the output ceiling, CHECKED because it cannot be SET. `end_turn` proves the model
    // stopped naturally; it does NOT prove the output stayed under the job's ceiling, and unlike the
    // api transport this one cannot put `max_tokens` on the wire.
    let ceiling = job.max_output_tokens;
    if let Some(used) = envelope.usage.as_ref().and_then(|u| u.output_tokens)
        && used > u64::from(ceiling)
    {
        // `job.kind`, not `job`: a `{job:?}` on the struct would print the model pin into a
        // user-facing error message.
        //
        // The bail NAMES THE KEY. Now that the ceiling is a budget the user set rather than a mirror of
        // an api limit, "you are over by N" without the one line that raises it is the remedy-less
        // error this file's own doctrine rejects — and on the cli path those tokens are already
        // generated and already billed, so the error is the only thing left that can be made useful.
        bail!(
            "claude -p produced {used} output tokens, over the {ceiling}-token ceiling for the {:?} \
             job; refusing to publish an artifact that exceeded its budget. Raise \
             {} in clyde.yml, or narrow the window with a shorter --since.\n{observations}",
            job.kind,
            job.kind.max_output_tokens_key()
        );
    }

    // GUARD 7: the model that actually ran.
    //
    // Observations are passed IN rather than wrapped around the returned error. `wrap_err` would make
    // the observations the outermost message, so a plain `{}` format (what most callers and the CLI's
    // top-level error printer use) would show only "binary: ... version: ..." and HIDE the actual
    // cause. Every other guard formats them inline; this one matches.
    check_model(&envelope.model_usage, job.model, observations)?;
    Ok(result)
}

/// Remediation appended to failures that could plausibly be an install, login, credential, or
/// model-selection problem — i.e. the ones the api transport would actually resolve. There is no
/// automatic fallback by design, so those failures must name the manual one.
///
/// Deliberately NOT appended to the truncation (Guard 4) or over-budget (Guard 6) bails: both are
/// per-job output-ceiling failures that the api path enforces identically, so pointing at `--llm api`
/// would be a remedy that does not remedy. See the module docs.
const ESCAPE_HATCH: &str =
    "try `claude` interactively to check the install and login, or pass --llm api to use ANTHROPIC_API_KEY";

impl CliTransport {
    /// What we OBSERVED, never a guessed cause. `which` proved only that a file of this name exists,
    /// so "not logged in" would be a guess dressed as a diagnosis.
    fn observations(&self) -> String {
        format!(
            "  binary:  {}\n  version: {} (minimum supported: {})",
            self.binary.display(),
            self.version,
            MIN_CLAUDE_VERSION
        )
    }

    /// The non-zero-exit report. Attempts an envelope parse purely to enrich the message: if the CLI
    /// managed to say what went wrong, that sentence is worth more than the exit code.
    fn exit_failure(&self, output: &std::process::Output) -> String {
        let stderr = preview(&output.stderr);
        let detail = parse_envelope(&output.stdout)
            .ok()
            .as_ref()
            .and_then(failure_detail)
            .map(|m| format!("\n  message: {m}"))
            .unwrap_or_default();
        format!(
            "claude -p failed (exit {})\n{}\n  stderr:  {}{}\n{}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
            self.observations(),
            if stderr.is_empty() { "<empty>" } else { &stderr },
            detail,
            ESCAPE_HATCH
        )
    }
}

/// The fully-specified child process. Built as DATA so the argv and the complete env can be asserted
/// in a unit test without spawning anything — `Command` exposes no getter for "was env_clear called",
/// so testing the built `Command` directly could not prove the child inherits nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    program: PathBuf,
    args: Vec<String>,
    /// The COMPLETE environment. `env_clear()` is always applied, so this is exactly what the child
    /// gets — not a set of overrides layered onto the parent's env.
    env: Vec<(String, String)>,
}

impl Spawn {
    fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        // BUILT, not inherited. See child_env().
        cmd.env_clear();
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }
}

/// The child's COMPLETE environment: an allowlist applied after `env_clear()`.
///
/// A denylist is the wrong shape here and the reason is a measured secret-exposure bug, not
/// tidiness. A live agent session on this host carries 13 `CLAUDE*` variables, three of which are
/// SECRETS — `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY`, `CLAUDE_COST_SLACK_APP_TOKEN`,
/// `CLAUDE_COST_SLACK_BOT_TOKEN`. An inherit-by-default child would receive an Anthropic ADMIN key
/// and two Slack tokens on every render, and a denylist would leak whatever secret-bearing variable
/// someone adds next. Fail closed: enumerate what the child gets.
///
/// Also excluded by construction: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`,
/// `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_EXECPATH`, `CLAUDE_TMPDIR`, `CLAUDE_EFFORT` — an
/// agent-invoked render must not present itself to the child as a nested session of the caller — and
/// `ANTHROPIC_API_KEY`, because `--llm cli` must mean what it says and cost attribution must never
/// silently flip to the key.
///
/// The proxy variables ([`PROXY_VARS`]) are the one addition, and they are enumerated by name for
/// the same fail-closed reason the rest of this list is.
fn child_env() -> Vec<(String, String)> {
    let mut env = Vec::new();
    // Measured 2026-07-24: an `env -i` child with NO env at all still authenticates, because the
    // runtime falls back to `getpwuid` for the home directory. HOME is passed anyway so the transport
    // never depends on that fallback — if it changed, the failure would present as "logged out",
    // which is the exact misdiagnosis this design fights.
    if let Some(home) = dirs::home_dir() {
        env.push(("HOME".into(), home.display().to_string()));
    }
    // Not needed to find `claude` (we exec the absolute resolved path), but without it the child
    // warns on stderr that it cannot find `bwrap`/`socat` and disables its own sandbox. Not a secret.
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".into(), path));
    }
    // An npm-installed Claude Code can print an update notice, and anything ahead of the JSON would
    // make a successful generation look like a malformed envelope. Belt; parse_envelope is suspenders.
    env.push(("NO_UPDATE_NOTIFIER".into(), "1".into()));
    // How the child reaches the network at all in a sandboxed environment. See PROXY_VARS.
    for name in PROXY_VARS {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => {
                debug!("child_env: forwarding {name} to the child");
                env.push((name.to_string(), value));
            }
            _ => {}
        }
    }
    env
}

/// The proxy variables forwarded to the child, ENUMERATED BY NAME.
///
/// Why they are needed: the Claude Code Bash sandbox advertises its egress proxy only through these
/// variables. With `env_clear()` and no passthrough the child `claude` attempts a direct connection,
/// the network namespace refuses it, and the render burns ~175 seconds before exiting 1 with an
/// `ENOTIMP` connection error that reads like a broken login (measured 2026-07-26; the same payload
/// renders fine outside the sandbox, so size was never the variable).
///
/// Why NOT a `*PROXY*` glob: this host also carries `CLOUDSDK_PROXY_PASSWORD`. A glob would hand a
/// credential to the child and reintroduce exactly the secret-leak class the allowlist exists to
/// prevent. A proxy ADDRESS is not a secret; a proxy PASSWORD is. Both cases of each name, because
/// the lowercase spellings are the conventional ones for `curl`-family tools and either may be the
/// one that is set.
const PROXY_VARS: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// Read `claude --version`, or a placeholder. Deliberately non-fatal: the version is for the operator
/// (logged on every render, named in every failure), so failing to read it must not fail the render.
fn probe_version(binary: &std::path::Path) -> String {
    let mut cmd = Command::new(binary);
    cmd.arg("--version");
    match proc::run_bounded("claude --version", &mut cmd, |e| eyre::eyre!("{e}")) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Ok(out) => {
            log::warn!(
                "CliTransport: `claude --version` exited {:?}; reporting version as unknown",
                out.status.code()
            );
            "unknown".into()
        }
        Err(e) => {
            log::warn!("CliTransport: could not read `claude --version`: {e}; reporting version as unknown");
            "unknown".into()
        }
    }
}

/// Parse the `--output-format json` envelope, tolerating leading noise on stdout.
///
/// Seeks the first `{` rather than assuming stdout begins with the JSON root: an update notice or any
/// other preamble ahead of the JSON would otherwise misreport a successful, already-billed generation
/// as a malformed envelope. That is a false negative on work we paid for, so it gets two guards
/// (`NO_UPDATE_NOTIFIER=1` in the child env is the other).
fn parse_envelope(stdout: &[u8]) -> Result<Envelope> {
    // NOT from_utf8_lossy: the envelope carries the ARTIFACT, and lossy decoding would silently
    // replace bytes inside the document we are about to publish. Reject non-UTF-8 loudly instead.
    let text = std::str::from_utf8(stdout).with_context(|| "claude -p produced non-UTF-8 stdout")?;
    let start = text
        .char_indices()
        .find(|(_, c)| *c == '{')
        .map(|(i, _)| i)
        .ok_or_else(|| {
            eyre::eyre!(
                "claude -p produced no JSON envelope on stdout ({} bytes, no `{{` found): {}\n{}",
                stdout.len(),
                preview(stdout),
                ESCAPE_HATCH
            )
        })?;
    // `get` is boundary-safe and `{` is ASCII, so this cannot split a multibyte char.
    let json = text
        .get(start..)
        .ok_or_else(|| eyre::eyre!("claude -p stdout ended unexpectedly while seeking the JSON envelope"))?;
    serde_json::from_str(json).with_context(|| {
        format!(
            "failed to parse the `claude -p --output-format json` envelope: {}",
            preview(json.as_bytes())
        )
    })
}

/// Assert the model that actually ran is the one we asked for, via a KEYED lookup.
///
/// NEVER a scan asserting every `modelUsage` entry matches. Measured 2026-07-24: the CLI makes an
/// internal `claude-haiku-4-5` sub-call on every render, so both real envelopes carried TWO entries
/// and a scan-and-compare-all would bail on every successful render.
///
/// The lookup tries the exact key first, then a normalized match, because the CLI keys entries by the
/// DATED id (`claude-haiku-4-5-20251001`) while the requested pin is usually undated.
/// `normalize_model_id` is `claude_pricing`'s existing public export, so the dated-suffix handling is
/// not reinvented here.
fn check_model(model_usage: &BTreeMap<String, ModelUsage>, requested: &str, observations: &str) -> Result<()> {
    let want = claude_pricing::normalize_model_id(requested);
    let entry = model_usage.get(requested).or_else(|| {
        model_usage
            .iter()
            .find(|(key, _)| claude_pricing::normalize_model_id(key) == want)
            .map(|(_, value)| value)
    });
    let Some(entry) = entry else {
        let saw: Vec<&str> = model_usage.keys().map(String::as_str).collect();
        bail!(
            "claude -p reported no usage for the requested model {requested}; it ran {:?} instead. \
             Refusing to publish an artifact from a model we did not pin.\n{observations}\n{ESCAPE_HATCH}",
            saw
        );
    };
    let got = entry.canonical_model.as_deref().unwrap_or_default();
    if claude_pricing::normalize_model_id(got) != want {
        bail!(
            "claude -p ran canonicalModel={got} but {requested} was requested; refusing to publish an \
             artifact from a substituted model\n{observations}\n{ESCAPE_HATCH}"
        );
    }
    debug!("check_model: requested={requested} canonical={got} ok");
    Ok(())
}

/// What a failing envelope says, in the order the CLI actually says it.
///
/// `claude` does NOT put its diagnosis on stderr; it puts it in the stdout envelope, and NOT always
/// under `error.message`. Measured 2026-07-26 on a connection failure: the envelope was
/// `{"is_error":true,"terminal_reason":"api_error","result":"API Error: Unable to connect to API
/// (ENOTIMP)"}` with **no `error` field at all**. Mining only `error.message` printed
/// `stderr: <empty>` and threw away the one sentence that answered the question, on both the
/// non-zero-exit path and Guard 2.
///
/// So the fallback chain is `error.message` -> `result` -> `terminal_reason`, and the reason is
/// appended when a message was found, because "api_error" classifies a sentence that does not
/// classify itself. Still observations only, never a guessed cause (see the module docs).
fn failure_detail(envelope: &Envelope) -> Option<String> {
    let message = envelope
        .error
        .as_ref()
        .and_then(|e| e.message.as_deref())
        // On this failure shape the diagnosis rides in `result`, the same field a SUCCESSFUL call
        // returns the artifact in. Bounded by `preview`, so a truncated artifact echoed back on a
        // half-failed call cannot become the error report.
        .or(envelope.result.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| preview(s.as_bytes()));
    let reason = envelope
        .terminal_reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let detail = match (message, reason) {
        (Some(m), Some(r)) => format!("{m} (terminal_reason: {r})"),
        (Some(m), None) => m,
        (None, Some(r)) => format!("terminal_reason: {r}"),
        (None, None) => return None,
    };
    debug!("failure_detail: bytes={}", detail.len());
    Some(detail)
}

/// Guard 2's last resort: the envelope claimed an error and carried no `error.message`, no `result`
/// and no `terminal_reason`. Named so the test that pins it cannot drift from the string.
const NO_DETAIL_IN_ENVELOPE: &str = "no error message, result, or terminal_reason in the envelope";

/// First [`STDERR_PREVIEW_BYTES`] of a byte stream, as trimmed lossy text. Display only, so lossy
/// decoding is correct here (unlike the envelope, which carries content).
fn preview(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(STDERR_PREVIEW_BYTES)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The `claude -p --output-format json` envelope.
///
/// Deliberately NOT `deny_unknown_fields`. This is a wire frame owned by another tool that will grow
/// fields — the real envelope already carries a dozen we ignore (`session_id`, `duration_ms`,
/// `ttft_ms`, `permission_denials`, ...) — so it is the documented forward-compatible-envelope
/// carve-out to the strict-serde house rule, not an oversight.
#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(rename = "modelUsage", default)]
    model_usage: BTreeMap<String, ModelUsage>,
    #[serde(default)]
    error: Option<ErrorBody>,
    /// The CLI's own classification of a terminal failure (`api_error`, ...). Present on failure
    /// envelopes that carry no `error` object at all, which is why [`failure_detail`] reads it.
    #[serde(default)]
    terminal_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelUsage {
    #[serde(rename = "canonicalModel", default)]
    canonical_model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
}

#[cfg(test)]
mod tests;
