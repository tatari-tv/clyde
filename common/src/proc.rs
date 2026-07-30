//! Bounded subprocess execution for external shell-outs.
//!
//! `report` drives three external binaries: `pandoc` (PDF), `marquee` (publish/whoami), and
//! `claude` (the keyless LLM transport). All of them need the same hygiene -- a wall-clock ceiling,
//! and a child that is killed AND reaped on overrun rather than left to hang the render -- so the
//! helpers live here instead of being reimplemented per call site.
//!
//! Moved here from `report::proc` (design `2026-07-29-excise-api-key.md` Phase 1) alongside
//! `common::llm::cli`, its only same-crate-boundary-crossing user at the time of the move.
//! [`run_bounded`], [`run_with_payload`], [`CLAUDE_TIMEOUT`], and [`SUBPROCESS_TIMEOUT`] widen from
//! `pub(crate)` to `pub` as a direct consequence of crossing the crate boundary; nothing about their
//! runtime behavior changed.
//!
//! Two shapes, deliberately separate, because their I/O constraints are opposites:
//!
//! - [`run_bounded`] pipes stdio and drains it after the child exits. Correct only when the
//!   combined output stays well under the OS pipe buffer (URLs, short stderr).
//! - `run_with_payload` (added with the `claude` transport) wires a large stdin payload and large
//!   captured output through temp FILES, so no pipe exists to fill and no drain can deadlock.

use eyre::{Context, Result, bail};
use log::debug;
use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Wall-clock ceiling for the `claude -p` LLM call. Its own const, deliberately NOT
/// [`SUBPROCESS_TIMEOUT`]: the 2026-07-24 keyless spike measured 145s (markdown) and 204s (html) on a
/// real 1,310-session month, so the 120s pandoc/marquee ceiling would have killed every real render.
///
/// 900s is ~4.4x the worst observed. The margin is deliberately wide because an overrun discards a
/// generation that has already been billed (~$3), which is the expensive direction to be wrong in.
pub const CLAUDE_TIMEOUT: Duration = Duration::from_secs(900);

/// Wall-clock ceiling for non-interactive external commands (pandoc, `marquee whoami`/`publish`).
/// A stalled network publish or a wedged pandoc must not hang `report render` indefinitely.
///
/// Deliberately NOT reused for the `claude` transport: Phase 0 measured 145s (markdown) and 204s
/// (html) on a real month, so this ceiling would kill every real render. That path has its own,
/// much wider const.
pub const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawn a non-interactive external command with piped stdio and a wall-clock ceiling
/// ([`SUBPROCESS_TIMEOUT`]); on timeout, kill and reap the child rather than blocking forever
/// (per the repo's subprocess-hygiene rule; mirrors `persona::whoami_via`). `spawn_err` maps a
/// spawn failure (e.g. binary-not-found) to a caller-specific message. Only for commands whose
/// combined output stays well under the OS pipe buffer (URLs, short stderr) -- large stdout must go
/// to a file, not a pipe, to avoid a fill-the-buffer deadlock.
pub fn run_bounded(
    label: &str,
    cmd: &mut Command,
    spawn_err: impl FnOnce(std::io::Error) -> eyre::Report,
) -> Result<Output> {
    debug!("proc::run_bounded: label={label} timeout={:?}", SUBPROCESS_TIMEOUT);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let status = match child.wait_timeout(SUBPROCESS_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            log::warn!("proc::run_bounded: {label} timed out after {SUBPROCESS_TIMEOUT:?}, killing child");
            let _ = child.kill();
            let _ = child.wait();
            bail!("{label} timed out after {SUBPROCESS_TIMEOUT:?}");
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{label}: failed while waiting: {e}");
        }
    };
    // `wait_timeout` has already reaped the child, so `wait_with_output()` (a second wait on the
    // same PID) would fail with ECHILD. Read the piped handles directly instead -- the process has
    // exited, and callers only route commands whose output stays well under the pipe buffer here
    // (large output, e.g. the pandoc PDF, goes to a file), so a post-exit drain cannot deadlock.
    // Mirrors `persona::whoami_via`.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout)
            .with_context(|| format!("failed to read stdout of {label}"))?;
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_end(&mut stderr)
            .with_context(|| format!("failed to read stderr of {label}"))?;
    }
    debug!(
        "proc::run_bounded: label={label} status={:?} stdout bytes={} stderr bytes={}",
        status.code(),
        stdout.len(),
        stderr.len()
    );
    Ok(Output { status, stdout, stderr })
}

/// Run `cmd` with `payload` on stdin and both output streams captured, all three wired to temp FILES
/// rather than pipes, under the [`CLAUDE_TIMEOUT`] wall clock. No pipe exists, so no pipe can fill
/// and no drain can deadlock. For large payloads and large output (the `claude -p` LLM call).
///
/// [`run_bounded`] cannot be reused here and the reason is a deadlock, not a preference. It sets
/// `stdin(Stdio::null())` -- there is nowhere to put a 500KB payload -- and it drains stdout only
/// AFTER the child exits. Writing a large payload into a pipe while not draining stdout deadlocks
/// (child blocks writing output, parent blocks writing input), and a post-exit drain deadlocks the
/// moment the child fills the ~64KB stdout pipe. This extends the pattern `write_pdf` already uses
/// for pandoc, whose own comment says large output goes to a file.
///
/// `spawn_err` maps a spawn failure (e.g. binary-not-found) to a caller-specific message.
pub fn run_with_payload(
    label: &str,
    cmd: &mut Command,
    payload: &str,
    spawn_err: impl FnOnce(std::io::Error) -> eyre::Report,
) -> Result<Output> {
    debug!(
        "proc::run_with_payload: label={label} payload bytes={} timeout={:?}",
        payload.len(),
        CLAUDE_TIMEOUT
    );

    // stdin: write the payload, flush, then reopen the path READ-ONLY so the child gets a handle
    // positioned at byte 0. Handing over the write handle would leave it at EOF.
    let mut stdin_file =
        tempfile::NamedTempFile::new().with_context(|| format!("failed to create stdin temp for {label}"))?;
    stdin_file
        .write_all(payload.as_bytes())
        .with_context(|| format!("failed to write stdin payload for {label}"))?;
    stdin_file
        .flush()
        .with_context(|| format!("failed to flush stdin payload for {label}"))?;
    let stdin_read = std::fs::File::open(stdin_file.path())
        .with_context(|| format!("failed to reopen stdin payload for {label}"))?;

    let stdout_file =
        tempfile::NamedTempFile::new().with_context(|| format!("failed to create stdout temp for {label}"))?;
    let stderr_file =
        tempfile::NamedTempFile::new().with_context(|| format!("failed to create stderr temp for {label}"))?;
    let stdout_path = stdout_file.path().to_path_buf();
    let stderr_path = stderr_file.path().to_path_buf();
    let stdout_handle = stdout_file
        .reopen()
        .with_context(|| format!("failed to reopen stdout temp for {label}"))?;
    let stderr_handle = stderr_file
        .reopen()
        .with_context(|| format!("failed to reopen stderr temp for {label}"))?;

    let mut child = cmd
        .stdin(Stdio::from(stdin_read))
        .stdout(Stdio::from(stdout_handle))
        .stderr(Stdio::from(stderr_handle))
        .spawn()
        .map_err(spawn_err)?;

    let status = match child.wait_timeout(CLAUDE_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            log::warn!("proc::run_with_payload: {label} timed out after {CLAUDE_TIMEOUT:?}, killing child");
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{label} timed out after {CLAUDE_TIMEOUT:?} and was killed; the generation did not \
                 complete, so no artifact was written"
            );
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{label}: failed while waiting: {e}");
        }
    };

    // The child has exited and both streams were files, so these reads cannot block.
    let stdout = std::fs::read(&stdout_path).with_context(|| format!("failed to read stdout of {label}"))?;
    let stderr = std::fs::read(&stderr_path).with_context(|| format!("failed to read stderr of {label}"))?;
    debug!(
        "proc::run_with_payload: label={label} status={:?} stdout bytes={} stderr bytes={}",
        status.code(),
        stdout.len(),
        stderr.len()
    );
    Ok(Output { status, stdout, stderr })
}

#[cfg(test)]
mod tests;
