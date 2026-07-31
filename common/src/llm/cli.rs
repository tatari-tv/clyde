//! The keyless transport: shell out to the locally installed `claude` CLI in headless print mode.
//!
//! Every clyde user already has a logged-in Claude Code -- that login is WHY they have sessions to
//! report on. This transport piggy-backs it, so clyde reads, stores, refreshes, and transmits NO
//! credential: the `claude` binary owns auth end to end. Same shape as the existing shell-outs to
//! `pandoc` and to `marquee` (which owns its own Okta tokens).
//!
//! Fail loud, never retry. This is the ONE LLM transport in the workspace (design
//! `2026-07-29-excise-api-key.md` Phase 4 deleted the api-key path), so EVERY failure is terminal --
//! logged out, non-zero exit, malformed envelope, non-`end_turn` stop, model mismatch, timeout.
//! Nothing retries and nothing falls back, because there is nowhere left to fall back to: a broken
//! login must surface, not hide behind a second credentialed path.
//!
//! Failures that could plausibly be fixed by installing or logging into `claude` carry the
//! [`ESCAPE_HATCH`]; the ones that could not, deliberately do not. A truncation or an over-budget
//! artifact is NOT one of them: a working install and login still cannot set a wire-level ceiling
//! over this transport, so appending the escape hatch there would send the reader to a fix that does
//! not fix anything. Suggesting a remedy that cannot work is worse than suggesting none.

use super::{Completion, Job, Kind, Transport, TransportError};
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
/// `--max-turns` -- and `--max-turns` is accepted but UNDOCUMENTED in 2.1.219, so it is exactly the
/// kind of flag that could vanish without a deprecation notice.
const MIN_CLAUDE_VERSION: &str = "2.1.219";

/// How much child stderr to quote in a failure. Enough to carry the real message, bounded so a
/// runaway stderr cannot become the error report.
const STDERR_PREVIEW_BYTES: usize = 500;

/// The CLI's own classification of a transport-level API failure, in [`Envelope::terminal_reason`].
/// Measured twice, four days apart: on a bogus-credential 401 and on a refused connection (design
/// Phase 0 Findings 8 and 9, and the dated fixture this file's tests already carry).
const TERMINAL_REASON_API_ERROR: &str = "api_error";

/// The HTTP statuses in [`Envelope::api_error_status`] that mean the transport, not the payload, is
/// the problem. Enumerated rather than "any 4xx": a 400 IS about this payload and stays per-session.
const HTTP_UNAUTHORIZED: u16 = 401;
const HTTP_FORBIDDEN: u16 = 403;
const HTTP_TOO_MANY_REQUESTS: u16 = 429;
/// Inclusive floor and exclusive ceiling of the 5xx range (upstream is down).
const HTTP_SERVER_ERROR_FLOOR: u16 = 500;
const HTTP_SERVER_ERROR_LIMIT: u16 = 600;

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
            eyre::eyre!("the `{CLAUDE_BINARY}` CLI was not found on PATH ({e}); install Claude Code and log in once")
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
                // isolation mechanism. A temp cwd only ever defeated PROJECT CLAUDE.md discovery --
                // user and global customizations still loaded -- so cwd is hygiene, not the control.
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
            env: child_env(job.kind),
        }
    }

    /// [`Transport::complete`] plus the token counts the CLI billed, for callers that PERSIST them.
    ///
    /// Inherent rather than on the trait: `sessions` holds a concrete `CliTransport` and needs the
    /// counts (they are durable columns), while `report`'s callers publish an artifact and never
    /// account for it. Widening `Transport::complete` for one caller would churn every existing
    /// implementation and test double for a value they discard. See [`Completion`].
    pub fn complete_with_usage(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<Completion> {
        let spawn = self.build_spawn(job, system, prompt);
        // The IDENTICAL fenced block the api transport puts in its user message, so the model sees
        // the same content in the same order on both transports. Only the channel differs. The LABEL
        // is the kind's, because it describes the payload: JSON facts for a slot or the judge, prose
        // for enrich and narrate.
        let payload = format!("```{}\n{json_body}\n```\n", job.kind.fence());
        info!(
            "CliTransport::complete: transport=cli job={job:?} binary={} version={} \
             payload bytes={}",
            self.binary.display(),
            self.version,
            payload.len()
        );

        let mut cmd = spawn.to_command();
        let binary = self.binary.clone();
        // A spawn failure means the binary we resolved a moment ago cannot be run AT ALL, which is
        // never about this payload -- so it is `Unavailable`, same class as a resolve failure.
        let output = proc::run_with_payload("claude -p", &mut cmd, &payload, move |e| {
            TransportError::Unavailable(format!(
                "failed to invoke the `claude` CLI at {}: {e}\n{ESCAPE_HATCH}",
                binary.display()
            ))
            .into()
        })?;

        // GUARD 1: exit status, checked BEFORE parsing. A logged-out `claude` exits non-zero and
        // prints to stderr WITHOUT emitting a JSON envelope, so parsing first would report
        // "malformed envelope" for what is really "you are logged out".
        //
        // Sweep-fatal, per the design's classification table ("logged out | non-zero exit, no
        // envelope | sweep-fatal"): a `claude` that cannot complete a run is not a property of the
        // payload, and one unattended sweep must not charge a durable attempt to every candidate.
        if !output.status.success() {
            return Err(TransportError::Unavailable(self.exit_failure(&output)).into());
        }

        let envelope = parse_envelope(&output.stdout)?;
        let completion = check_envelope(envelope, job, &self.observations())?;
        debug!(
            "CliTransport::complete_with_usage: job={job:?} ok result bytes={} tokens_in={} tokens_out={}",
            completion.text.len(),
            completion.tokens_in,
            completion.tokens_out
        );
        Ok(completion)
    }
}

impl Transport for CliTransport {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String> {
        Ok(self.complete_with_usage(job, system, prompt, json_body)?.text)
    }
}

/// Guards 2-8, applied to an already-parsed envelope from an already-successful exit.
///
/// Pure, and separate from [`CliTransport::complete`] so every failure mode is driven by a recorded
/// envelope fixture in tests rather than requiring a real `claude` subprocess. Returns the validated
/// artifact text. Every guard bails loudly; none of them degrade the artifact.
fn check_envelope(envelope: Envelope, job: Job<'_>, observations: &str) -> Result<Completion> {
    debug!(
        "check_envelope: job={job:?} is_error={} subtype={:?} stop_reason={:?} api_error_status={:?}",
        envelope.is_error, envelope.subtype, envelope.stop_reason, envelope.api_error_status
    );

    // GUARD 2: the CLI's own error message, forwarded VERBATIM. An expired token produces a perfectly
    // well-formed envelope saying exactly what is wrong; reporting that as a generic failure throws
    // away the one useful sentence we were given.
    //
    // This is also where the sweep-fatal split lives, and it is the reason the split cannot be done on
    // exit status: this envelope arrives at exit 0 (design G5). `is_sweep_fatal` reads the STRUCTURED
    // fields; nothing here matches prose.
    if envelope.is_error {
        let detail = failure_detail(&envelope).unwrap_or_else(|| NO_DETAIL_IN_ENVELOPE.to_string());
        let report = format!("claude -p reported an error: {detail}\n{observations}\n{ESCAPE_HATCH}");
        if is_sweep_fatal(&envelope) {
            return Err(TransportError::Unavailable(report).into());
        }
        bail!("{report}");
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

    // GUARD 6: usage MUST be present on an otherwise-successful envelope. The CLI bills the payload
    // as a 1h cache write (`cache_creation_input_tokens`), so reading an absent `usage` as a zero
    // would make a token-budget gate a no-op that silently never trips (design
    // `2026-07-29-excise-api-key.md` Phase 2, Data Model; measured Phase 0 Finding 7). A failed call
    // is safer than a silently-zero token count.
    let usage = envelope.usage.as_ref().ok_or_else(|| {
        eyre::eyre!(
            "claude -p returned a successful envelope for the {:?} job with no usage; refusing to \
             record a token count that was never observed.\n{observations}\n{ESCAPE_HATCH}",
            job.kind
        )
    })?;
    debug!(
        "check_envelope: job={:?} tokens_in={} tokens_out={}",
        job.kind,
        usage.tokens_in(),
        usage.tokens_out()
    );

    // GUARD 7: the output ceiling, CHECKED because it cannot be SET -- and checked ONLY for a kind
    // whose ceiling the user can actually set. `end_turn` proves the model stopped naturally; it does
    // NOT prove the output stayed under the job's ceiling, and unlike the api transport this one cannot
    // put `max_tokens` on the wire.
    //
    // `max_output_tokens_key()` is the gate, so the two facts cannot diverge: a kind with no config key
    // has no output BUDGET, only a const, and there would be no line to name in the bail below. For
    // `Kind::Enrich`/`Kind::Narrate` the count is dominated by CLI-side reasoning that never reaches
    // `result` (measured 5,798 and 678 tokens against a 512 const, and it does not track payload size,
    // so no low ceiling is safe) -- so `stop_reason == end_turn`, checked by Guard 4 above, is the whole
    // truncation contract for them (design Phase 0 Finding 3 + Finding 10).
    if let Some(ceiling_key) = job.kind.max_output_tokens_key() {
        let ceiling = job.max_output_tokens;
        let used = usage.tokens_out();
        if used > u64::from(ceiling) {
            // `job.kind`, not `job`: a `{job:?}` on the struct would print the model pin into a
            // user-facing error message.
            //
            // The bail NAMES THE KEY. Now that the ceiling is a budget the user set rather than a mirror
            // of an api limit, "you are over by N" without the one line that raises it is the
            // remedy-less error this file's own doctrine rejects -- and on the cli path those tokens are
            // already generated and already billed, so the error is the only thing left that can be
            // made useful.
            bail!(
                "claude -p produced {used} output tokens, over the {ceiling}-token ceiling for the {:?} \
                 job; refusing to publish an artifact that exceeded its budget. Raise \
                 {ceiling_key} in clyde.yml, or narrow the window with a shorter --since.\n{observations}",
                job.kind,
            );
        }
    }

    // GUARD 8: the model that actually ran.
    //
    // Observations are passed IN rather than wrapped around the returned error. `wrap_err` would make
    // the observations the outermost message, so a plain `{}` format (what most callers and the CLI's
    // top-level error printer use) would show only "binary: ... version: ..." and HIDE the actual
    // cause. Every other guard formats them inline; this one matches.
    check_model(&envelope.model_usage, job.model, observations)?;
    Ok(Completion {
        text: result,
        tokens_in: usage.tokens_in(),
        tokens_out: usage.tokens_out(),
    })
}

/// Whether a failing envelope means the TRANSPORT cannot serve requests, rather than that THIS payload
/// failed. Structured only: an HTTP status and the CLI's own terminal classification, never prose.
///
/// The two rows are each measured (design Phase 0):
/// - a status of 401/403 (auth), 429 (rate limit) or 5xx (upstream down). Finding 8 measured
///   `api_error_status: 401` on a rejected credential under the exact argv this transport builds.
/// - `terminal_reason: "api_error"` with NO status, which is the network case: Finding 9 measured a
///   refused connection returning `api_error` with `api_error_status: null`, and this file's dated
///   2026-07-26 fixture carries the same shape.
///
/// Any OTHER status stays per-session, because a 400 is about the request we just sent. Everything the
/// later guards catch (malformed envelope, bad schema, empty result, model mismatch, non-`end_turn`
/// stop, over-ceiling) stays per-session too: each is a property of one call's reply.
fn is_sweep_fatal(envelope: &Envelope) -> bool {
    let fatal = match envelope.api_error_status {
        Some(status) => {
            matches!(status, HTTP_UNAUTHORIZED | HTTP_FORBIDDEN | HTTP_TOO_MANY_REQUESTS)
                || (HTTP_SERVER_ERROR_FLOOR..HTTP_SERVER_ERROR_LIMIT).contains(&status)
        }
        // Belt-and-braces for a status-less API failure. Reached only when the CLI itself classified
        // the failure as `api_error`, so it is not a catch-all for every message-less error envelope.
        None => envelope.terminal_reason.as_deref() == Some(TERMINAL_REASON_API_ERROR),
    };
    debug!(
        "is_sweep_fatal: api_error_status={:?} terminal_reason={:?} fatal={fatal}",
        envelope.api_error_status, envelope.terminal_reason
    );
    fatal
}

/// Remediation appended to failures that could plausibly be an install, login, credential, or
/// model-selection problem. There is no fallback transport, so checking the install and login is the
/// one manual remedy left to name.
///
/// Deliberately NOT appended to the truncation (Guard 4) or over-budget (Guard 7) bails: both are
/// per-job output-ceiling failures that a working install and login cannot fix, so appending this
/// would be a remedy that does not remedy. See the module docs.
const ESCAPE_HATCH: &str = "try `claude` interactively to check the install and login";

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
/// in a unit test without spawning anything -- `Command` exposes no getter for "was env_clear called",
/// so testing the built `Command` directly could not prove the child inherits nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    program: PathBuf,
    args: Vec<String>,
    /// The COMPLETE environment. `env_clear()` is always applied, so this is exactly what the child
    /// gets -- not a set of overrides layered onto the parent's env.
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
/// SECRETS -- `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY`, `CLAUDE_COST_SLACK_APP_TOKEN`,
/// `CLAUDE_COST_SLACK_BOT_TOKEN`. An inherit-by-default child would receive an Anthropic ADMIN key
/// and two Slack tokens on every render, and a denylist would leak whatever secret-bearing variable
/// someone adds next. Fail closed: enumerate what the child gets.
///
/// Also excluded by construction: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`,
/// `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_EXECPATH`, `CLAUDE_TMPDIR`, `CLAUDE_EFFORT` -- an
/// agent-invoked render must not present itself to the child as a nested session of the caller -- and
/// `ANTHROPIC_API_KEY`, because clyde handles no key at all and must never forward one to the child.
///
/// The proxy variables ([`PROXY_VARS`]) are the one addition, and they are enumerated by name for
/// the same fail-closed reason the rest of this list is.
///
/// Keyed on `kind` for ONE reason: [`MAX_THINKING_TOKENS`], set for [`Kind::Enrich`] alone. This
/// function is shared by every job on the transport, so setting it unconditionally would silently
/// change what `report render` and `report eval` produce.
fn child_env(kind: Kind) -> Vec<(String, String)> {
    let mut env = Vec::new();
    // Measured 2026-07-24 ON LINUX: an `env -i` child with NO env at all still authenticates, because
    // the runtime falls back to `getpwuid` for the home directory. HOME is passed anyway so the
    // transport never depends on that fallback -- if it changed, the failure would present as "logged
    // out", which is the exact misdiagnosis this design fights.
    //
    // That measurement did NOT generalize, and the correction is [`USER_VAR`] below: it held on the
    // maintainer's Linux host and was read as universal, which is precisely the failure mode the
    // paragraph above warns about.
    if let Some(home) = dirs::home_dir() {
        env.push(("HOME".into(), home.display().to_string()));
    }
    // REQUIRED on macOS, where the child resolves its OAuth credentials through the login Keychain and
    // needs to know which user it is running as. Without it `claude -p` exits reporting "Not logged in"
    // on a host whose `claude auth status` is healthy and whose interactive `claude -p` succeeds, so the
    // symptom points at the login and not at this allowlist -- three teammates lost a run to it
    // (reported 2026-07-31, isolated with `env -i` by stripping only this variable).
    //
    // Forwarded, not synthesized: `getpwuid` would give the same answer on the happy path, but a `sudo`
    // or `su` context is exactly where the two disagree, and the child must act as the invoking user.
    // Not a secret -- a username, already visible in every process listing and in `HOME` above.
    match std::env::var(USER_VAR) {
        Ok(user) if !user.is_empty() => {
            debug!("child_env: forwarding {USER_VAR} to the child");
            env.push((USER_VAR.into(), user));
        }
        // Linux does not need it (the `getpwuid` fallback above), so an unset `USER` is not fatal here.
        // It is still worth a line in the log, because on macOS this is the difference between a
        // successful render and a "Not logged in" that looks like an auth problem.
        _ => debug!("child_env: {USER_VAR} unset or empty in the parent; not forwarding"),
    }
    // Not needed to find `claude` (we exec the absolute resolved path), but without it the child
    // warns on stderr that it cannot find `bwrap`/`socat` and disables its own sandbox. Not a secret.
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".into(), path));
    }
    // An npm-installed Claude Code can print an update notice, and anything ahead of the JSON would
    // make a successful generation look like a malformed envelope. Belt; parse_envelope is suspenders.
    env.push(("NO_UPDATE_NOTIFIER".into(), "1".into()));
    // Reasoning off, for the enrichment sweep ONLY. Set by clyde, never forwarded from the parent, so
    // the `env_clear()` allowlist posture is unchanged.
    if kind == Kind::Enrich {
        debug!("child_env: {MAX_THINKING_TOKENS}={THINKING_DISABLED} for {kind:?}");
        env.push((MAX_THINKING_TOKENS.into(), THINKING_DISABLED.into()));
    }
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

/// The variable that disables the CLI's own reasoning pass, and the value that disables it.
///
/// `claude` 2.1.220 treats thinking as enabled iff the value is `> 0`, so `0` turns it off. Set for
/// [`Kind::Enrich`] and NOTHING else, and the exclusions are each measured (design Phase 0):
/// - Enrich: 67% cheaper on a p50 payload, ~9x faster (6s vs 52s), output collapses from 5,798 to 140
///   tokens, tags and summary equal or better (Finding 12). Enrich runs once per session over hundreds
///   of sessions, so the saving compounds.
/// - NOT [`Kind::Narrate`]: measured 3 runs per mode on identical facts, the flag DETERMINISTICALLY
///   flips the verdict (inefficient 3/3 with reasoning, efficient 3/3 without). That is a change to
///   what narrate produces, which the design's Non-Goal excludes, and one interactive call had no cost
///   case to justify it (Finding 13).
/// - NOT [`Kind::Slot`]/[`Kind::Judge`]: unrequested and unmeasured; it would silently change what
///   `report render` and `report eval` produce.
///
/// It is undocumented in the `claude` binary, so the failure mode is COST, not correctness: if a future
/// release stops honoring it, enrichment still succeeds and simply gets ~3x dearer and ~9x slower. The
/// canary is ~140 output tokens and ~6s per enrich call (Finding 14).
const MAX_THINKING_TOKENS: &str = "MAX_THINKING_TOKENS";
const THINKING_DISABLED: &str = "0";

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
/// The invoking user's name, forwarded to the child so macOS can find its Keychain-backed OAuth
/// credentials. See the forwarding site in [`child_env`] for why this is required there and merely
/// belt-and-braces on Linux.
const USER_VAR: &str = "USER";

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
/// So the fallback chain is `error.message` -> `errors[].message` -> `result` -> `terminal_reason`, and
/// the reason is appended when a message was found, because "api_error" classifies a sentence that does
/// not classify itself. Still observations only, never a guessed cause (see the module docs).
fn failure_detail(envelope: &Envelope) -> Option<String> {
    let message = envelope
        .error
        .as_ref()
        .and_then(|e| e.message.as_deref())
        // The plural spelling, preferred over `result` for the same reason the singular is: it is the
        // CLI's own sentence about what went wrong. First non-empty message wins; the rest would be
        // noise in a one-line report.
        .or_else(|| {
            envelope
                .errors
                .iter()
                .filter_map(|e| e.message.as_deref())
                .find(|m| !m.trim().is_empty())
        })
        // Trim and reject empty BEFORE the fallback, not after. `.or()` fires only on `None`, so an
        // `error: {"message": ""}` short-circuited it and the `filter` then threw the empty string
        // away -- leaving no detail at all while a populated `result` sat right there unread. An
        // empty message carries the same information as an absent one and must fall back the same.
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // On this failure shape the diagnosis rides in `result`, the same field a SUCCESSFUL call
        // returns the artifact in. Bounded by `preview`, so a truncated artifact echoed back on a
        // half-failed call cannot become the error report.
        .or_else(|| envelope.result.as_deref().map(str::trim).filter(|s| !s.is_empty()))
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
/// fields -- the real envelope already carries a dozen we ignore (`session_id`, `duration_ms`,
/// `ttft_ms`, `permission_denials`, ...) -- so it is the documented forward-compatible-envelope
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
    /// The SAME thing, in the plural: some failure shapes emit an `errors` array instead of a singular
    /// `error` object. Deserialized alongside it rather than instead of it, because both spellings are
    /// in the wire format and reading only one silently drops the other's message.
    #[serde(default)]
    errors: Vec<ErrorBody>,
    /// The CLI's own classification of a terminal failure (`api_error`, ...). Present on failure
    /// envelopes that carry no `error` object at all, which is why [`failure_detail`] reads it, and one
    /// of the two structured signals [`is_sweep_fatal`] classifies on.
    #[serde(default)]
    terminal_reason: Option<String>,
    /// The upstream HTTP status of an API failure, when there was one. THE typed auth discriminator:
    /// measured 401 on a rejected credential and `null` on a refused connection, both under the exact
    /// argv this transport builds (design Phase 0 Findings 8 and 9). Marked `@internal` by the CLI and
    /// propagated only when `subtype == "success"` -- which a failing envelope satisfies, because it
    /// sets `subtype: "success"` and `is_error: true` at once.
    #[serde(default)]
    api_error_status: Option<u16>,
}

/// Token accounting for one `claude -p` call.
///
/// Forward-compatible on purpose, same as [`Envelope`]: the real envelope carries fields this struct
/// does not name (`service_tier`, `cache_creation`), and `#[serde(default)]` on every field also
/// tolerates a bucket the CLI omits entirely rather than sending as zero -- measured on the largest
/// Phase 0 payload, whose `usage` carries no `cache_read_input_tokens` key at all.
#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

impl Usage {
    /// Every input-token bucket the CLI bills, summed. `input_tokens` alone reads near-zero for a
    /// large payload, because the CLI bills the payload itself as a 1h cache write
    /// (`cache_creation_input_tokens`), not as plain input (measured Phase 0 Finding 7).
    fn tokens_in(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }

    /// The model's output tokens, including any reasoning that never reaches `result`.
    fn tokens_out(&self) -> u64 {
        self.output_tokens.unwrap_or(0)
    }
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
