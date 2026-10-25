//! `clyde sessions resume`: plan the resume from a catalog record, then exec `claude --resume`.

use std::path::{Path, PathBuf};

use colored::Colorize;
use eyre::{Context, Result};
use report::fmt::short_id;
use sessions::{Db, SessionRecord};

use crate::cli::ResumeArgs;
use crate::{lazy_reindex, resolve_one_or_exit};

/// What `resume` should do, computed from a resolved [`SessionRecord`] by [`plan_resume`]. Returned
/// to `main` (which drops the `Db` and then acts on it) so the decision stays a pure, unit-testable
/// function and no SQLite handle is held across the exec.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResumeAction {
    /// The live transcript exists: `chdir` into `dir` and exec `claude --resume <id> [extra]`.
    Launch {
        dir: PathBuf,
        id: String,
        extra: Vec<String>,
    },
    /// The record has no recorded cwd, so there is no directory to resume in.
    NoCwd { id: String },
    /// The recorded cwd is gone (deleted/moved) or is not a directory.
    MissingDir { dir: PathBuf },
    /// Only a durable staged copy exists; `claude --resume` cannot attach to it.
    StagedOnly { staged: PathBuf },
    /// Neither the live transcript nor a staged copy exists: fully TTL-reaped.
    Reaped,
}

/// Pure decision: maps a resolved record to what `resume` should do. Unit-tested directly (Phase 3).
///
/// Branch precedence:
/// 1. No recorded `cwd` -> `NoCwd` (nothing to resume in place).
/// 2. `cwd` present but not an existing directory -> `MissingDir`.
/// 3. Live `transcript_path` exists -> `Launch` (resumable). This mirrors the existence-based
///    decision in `sessions/src/mcp.rs` (`open_result_for`): prefer the live transcript on disk,
///    robust to a TTL reap between lookup and use, NOT the possibly-stale `archived` flag.
/// 4. else a `staged_path` that exists -> `StagedOnly` (durable copy only; not resumable).
/// 5. else -> `Reaped`.
pub(crate) fn plan_resume(rec: &SessionRecord, extra: Vec<String>) -> ResumeAction {
    let Some(cwd) = rec.cwd.as_deref() else {
        return ResumeAction::NoCwd {
            id: rec.session_id.clone(),
        };
    };
    let dir = PathBuf::from(cwd);
    if !dir.is_dir() {
        return ResumeAction::MissingDir { dir };
    }
    if rec.transcript_path.exists() {
        ResumeAction::Launch {
            dir,
            id: rec.session_id.clone(),
            extra,
        }
    } else if let Some(staged) = rec.staged_path.clone().filter(|p| p.exists()) {
        ResumeAction::StagedOnly { staged }
    } else {
        ResumeAction::Reaped
    }
}

/// Resolve the id (honoring `--no-reindex`), fetch the record, and compute the [`ResumeAction`].
/// Does NOT act on the action: `main` drops the `Db` first, then calls [`run_resume_action`].
pub(crate) fn cmd_resume(db: &Db, args: ResumeArgs) -> Result<ResumeAction> {
    // `extra` is arbitrary passthrough forwarded verbatim to claude and may contain sensitive
    // values (tokens, endpoints); log only its length, never the contents.
    log::debug!(
        "cmd_resume: id={} no_reindex={} extra_count={}",
        args.id,
        args.no_reindex,
        args.extra.len()
    );
    lazy_reindex(db, args.no_reindex);
    let id = resolve_one_or_exit(db, &args.id)?;
    let rec = db
        .get(&id)?
        .ok_or_else(|| eyre::eyre!("session {id} resolved but not found in the catalog"))?;
    Ok(plan_resume(&rec, args.extra))
}

/// Execute the planned [`ResumeAction`]. `Launch` replaces this process with `claude` (never returns
/// on unix success); every other variant prints a specific stderr message and exits non-zero.
pub(crate) fn run_resume_action(action: ResumeAction) -> Result<()> {
    match action {
        // `launch_resume` returns only on failure (exec replaced the image, or the non-unix branch
        // exited with claude's status). Render that failure as the same `✗` line + non-zero exit as
        // the other variants, rather than propagating an `Err` that `main` would print with an
        // eyre `Location:` backtrace.
        ResumeAction::Launch { dir, id, extra } => {
            if let Err(e) = launch_resume(&dir, &id, &extra) {
                eprintln!("{} {e}", "✗".red());
                std::process::exit(1);
            }
            Ok(())
        }
        ResumeAction::NoCwd { id } => {
            eprintln!(
                "{} session {} has no recorded cwd; cannot resume in place",
                "✗".red(),
                short_id(&id),
            );
            std::process::exit(1);
        }
        ResumeAction::MissingDir { dir } => {
            eprintln!(
                "{} recorded cwd is not a usable directory: {}",
                "✗".red(),
                dir.display(),
            );
            std::process::exit(1);
        }
        ResumeAction::StagedOnly { staged } => {
            eprintln!(
                "{} only a staged copy exists ({}); the live transcript is gone, so `claude --resume` cannot attach",
                "✗".red(),
                staged.display(),
            );
            std::process::exit(1);
        }
        ResumeAction::Reaped => {
            eprintln!(
                "{} session transcript is gone (TTL-reaped); nothing to resume",
                "✗".red(),
            );
            std::process::exit(1);
        }
    }
}

/// Resolve the `claude` executable to an ABSOLUTE path against the CURRENT process's `PATH`, before
/// any `chdir`. `launch_resume` changes into the session's directory before launching claude;
/// resolving first means the lookup cannot be influenced by that directory (a planted `./claude` on
/// a `PATH` carrying a relative entry can't shadow the real binary). Also yields a clear error when
/// `claude` is not installed, instead of a post-chdir exec failure.
///
/// `which` can return a RELATIVE path when the matching `PATH` entry is itself relative; that path
/// would otherwise be re-resolved against the SESSION directory after `current_dir(dir)`, defeating
/// the resolve-before-chdir guarantee. So a non-absolute result is `canonicalize()`d here (against
/// the current cwd, which is still the original directory at this point) to anchor it before the
/// chdir. The returned `PathBuf` is guaranteed absolute.
fn resolve_claude() -> Result<PathBuf> {
    let found = which::which("claude").map_err(|err| eyre::eyre!("could not find `claude` on PATH: {err}"))?;
    if found.is_absolute() {
        return Ok(found);
    }
    // Relative result (relative PATH entry): anchor it to the current (pre-chdir) directory now, or
    // a later `current_dir(dir)` would re-resolve it against the session dir.
    found
        .canonicalize()
        .with_context(|| format!("failed to canonicalize `claude` path {}", found.display()))
}

/// Replace the clyde process with `claude --resume <id> [extra...]`, running in `dir` so Claude
/// resolves the session's `~/.claude/projects/<slug>`. On unix this never returns on success (exec
/// replaces the image); it returns only if claude could not be launched.
#[cfg(unix)]
fn launch_resume(dir: &Path, id: &str, extra: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    // `extra` may carry sensitive passthrough values; log only its length.
    log::debug!(
        "launch_resume: dir={} id={} extra_count={}",
        dir.display(),
        id,
        extra.len()
    );
    let claude = resolve_claude()?;
    let mut cmd = std::process::Command::new(&claude);
    cmd.current_dir(dir).arg("--resume").arg(id).args(extra);
    // `current_dir(dir).exec()` performs the chdir before execvp; `claude` is already an absolute
    // path (resolved against the parent's PATH above), so the chdir cannot change which binary runs.
    let err = cmd.exec(); // returns only on failure
    Err(eyre::eyre!(
        "failed to exec {} in {}: {err}",
        claude.display(),
        dir.display()
    ))
}

/// Non-unix: no `exec`. Spawn claude inheriting stdio, wait, and exit with its status code (or a
/// fixed non-zero when terminated without a code).
#[cfg(not(unix))]
fn launch_resume(dir: &Path, id: &str, extra: &[String]) -> Result<()> {
    // `extra` may carry sensitive passthrough values; log only its length.
    log::debug!(
        "launch_resume: dir={} id={} extra_count={}",
        dir.display(),
        id,
        extra.len()
    );
    let claude = resolve_claude()?;
    let status = std::process::Command::new(&claude)
        .current_dir(dir)
        .arg("--resume")
        .arg(id)
        .args(extra)
        .status()
        .map_err(|err| eyre::eyre!("failed to launch {} in {}: {err}", claude.display(), dir.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests;
