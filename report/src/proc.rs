//! Bounded subprocess execution for `report`'s shell-outs.
//!
//! `report` drives three external binaries: `pandoc` (PDF), `marquee` (publish/whoami), and
//! `claude` (the keyless LLM transport). All of them need the same hygiene -- a wall-clock ceiling,
//! and a child that is killed AND reaped on overrun rather than left to hang the render -- so the
//! helpers live here instead of being reimplemented per call site.
//!
//! Two shapes, deliberately separate, because their I/O constraints are opposites:
//!
//! - [`run_bounded`] pipes stdio and drains it after the child exits. Correct only when the
//!   combined output stays well under the OS pipe buffer (URLs, short stderr).
//! - `run_with_payload` (added with the `claude` transport) wires a large stdin payload and large
//!   captured output through temp FILES, so no pipe exists to fill and no drain can deadlock.

use eyre::{Context, Result, bail};
use log::debug;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Wall-clock ceiling for non-interactive external commands (pandoc, `marquee whoami`/`publish`).
/// A stalled network publish or a wedged pandoc must not hang `report render` indefinitely.
///
/// Deliberately NOT reused for the `claude` transport: Phase 0 measured 145s (markdown) and 204s
/// (html) on a real month, so this ceiling would kill every real render. That path has its own,
/// much wider const.
pub(crate) const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawn a non-interactive external command with piped stdio and a wall-clock ceiling
/// ([`SUBPROCESS_TIMEOUT`]); on timeout, kill and reap the child rather than blocking forever
/// (per the repo's subprocess-hygiene rule; mirrors `persona::whoami_via`). `spawn_err` maps a
/// spawn failure (e.g. binary-not-found) to a caller-specific message. Only for commands whose
/// combined output stays well under the OS pipe buffer (URLs, short stderr) — large stdout must go
/// to a file, not a pipe, to avoid a fill-the-buffer deadlock.
pub(crate) fn run_bounded(
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
    // same PID) would fail with ECHILD. Read the piped handles directly instead — the process has
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
