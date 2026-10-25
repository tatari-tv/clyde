//! `clyde sessions resume`: plan the resume from a catalog record, then exec `claude --resume`.

mod pick;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use colored::Colorize;
use eyre::{Context, Result};
use report::fmt::short_id;
use sessions::{Db, Fallback, SearchResults, SessionRecord, SortBy};

use crate::cli::ResumeArgs;
use crate::{display_title, lazy_reindex, print_hits, record_location};
use pick::PickOutcome;

/// Exit status for a cancelled picker, matching fzf's own Esc / Ctrl-C code.
const EXIT_CANCELLED: i32 = 130;

/// What a resume query resolved to, before any picking.
#[derive(Debug)]
enum Resolution {
    /// Resume this id directly. `announce` prints the one-hit stderr line.
    One {
        id: String,
        announce: bool,
    },
    /// Search candidates for the picker; the full `SearchResults` is kept so no-TTY prints it as-is.
    Pick {
        results: SearchResults,
        header: String,
    },
    NoMatch,
}

/// The single arg when the query is exactly one arg made only of hex digits (either case) and `-`.
/// `resolve_id`'s `LIKE` ignores ASCII case, so `B8024AF2` matches as it did before. No length gate: the
/// arg is an id only if [`resolution_from_id`] then finds exactly one session. Rejecting every
/// other char also keeps `%` and `_` away from `Db::resolve_id`'s unescaped `LIKE`.
fn looks_like_id(query: &[String]) -> Option<&str> {
    match query {
        [arg] if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_hexdigit() || c == '-') => Some(arg.as_str()),
        _ => None,
    }
}

/// `Some` only when `resolve_id` found exactly one session; 0 or 2+ matches mean the arg is not an
/// id and the caller falls through to search. Only an arg equal to the full id resumes silently.
fn resolution_from_id(matches: &[String], arg: &str) -> Option<Resolution> {
    match matches {
        [id] => Some(Resolution::One {
            id: id.clone(),
            announce: !id.eq_ignore_ascii_case(arg),
        }),
        _ => None,
    }
}

/// Map a search response to a [`Resolution`]. Auto-resume only a single AND hit from an untruncated
/// list: a lone OR-fallback hit matched only some words, and a truncated list may hide others.
fn resolution_from_search(results: SearchResults, query: &str) -> Resolution {
    let n = results.results.len();
    if n == 0 {
        return Resolution::NoMatch;
    }
    if !results.truncated
        && results.fallback.is_none()
        && let [hit] = results.results.as_slice()
    {
        return Resolution::One {
            id: hit.record.session_id.clone(),
            announce: true,
        };
    }
    let header = if results.truncated {
        format!("showing the first {n} matches (truncated); refine the query")
    } else if results.fallback == Some(Fallback::Or) {
        "no session matched all terms; showing any-term matches".to_string()
    } else {
        format!("{n} sessions match {query:?}")
    };
    Resolution::Pick { results, header }
}

/// Id path first (one exact `resolve_id` hit), else the same search `session search` runs:
/// archived excluded, default limit, relevance order.
fn resolve_query(db: &Db, query: &[String]) -> Result<Resolution> {
    log::debug!("resolve_query: query={query:?}");
    if let Some(arg) = looks_like_id(query) {
        let matches = db.resolve_id(arg)?;
        if let Some(resolution) = resolution_from_id(&matches, arg) {
            log::debug!("resolve_query: {arg:?} resolved as an id");
            return Ok(resolution);
        }
        log::debug!(
            "resolve_query: {arg:?} matched {} ids, not exactly one; searching instead",
            matches.len()
        );
    }
    let joined = query.join(" ");
    let results = db.search(&joined, None, false, SortBy::Relevance)?;
    log::debug!(
        "resolve_query: search count={} fallback={:?} truncated={}",
        results.count,
        results.fallback,
        results.truncated
    );
    Ok(resolution_from_search(results, &joined))
}

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
    /// The user dismissed the picker (Esc / Ctrl-C). Not an `Err`: `main` maps every `Err` to exit
    /// 1, and a cancel exits 130.
    Cancelled,
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

/// Resolve the query (honoring `--no-reindex`), fetch the record, and compute the [`ResumeAction`].
/// Does NOT act on the action: `main` drops the `Db` first, then calls [`run_resume_action`].
/// Zero matches, several candidates without a TTY, and a picker failure print and exit 1 here;
/// several candidates on a TTY go through the fzf picker.
pub(crate) fn cmd_resume(db: &Db, args: ResumeArgs) -> Result<ResumeAction> {
    // `extra` is arbitrary passthrough forwarded verbatim to claude and may contain sensitive
    // values (tokens, endpoints); log only its length, never the contents.
    log::debug!(
        "cmd_resume: query={:?} no_reindex={} extra_count={}",
        args.query,
        args.no_reindex,
        args.extra.len()
    );
    lazy_reindex(db, args.no_reindex);
    let (id, announce) = match resolve_query(db, &args.query)? {
        Resolution::One { id, announce } => (id, announce),
        Resolution::NoMatch => {
            eprintln!("{} no session matches {:?}", "✗".red(), args.query.join(" "));
            std::process::exit(1);
        }
        Resolution::Pick { results, header } => {
            log::debug!("cmd_resume: {} candidates, header={header:?}", results.results.len());
            // Gate on stdin only: fzf draws on /dev/tty, so a piped stdout does not matter.
            if !std::io::stdin().is_terminal() {
                print_hits(&results);
                eprintln!("{} {}; pass an id", "✗".red(), candidates_phrase(results.results.len()));
                std::process::exit(1);
            }
            match pick::pick(&results, &header) {
                Ok(PickOutcome::Picked(id)) => (id, false),
                Ok(PickOutcome::Cancelled) => return Ok(ResumeAction::Cancelled),
                Err(e) => {
                    eprintln!("{} {e:#}", "✗".red());
                    std::process::exit(1);
                }
            }
        }
    };
    let rec = db
        .get(&id)?
        .ok_or_else(|| eyre::eyre!("session {id} resolved but not found in the catalog"))?;
    if announce {
        eprintln!(
            "→ resuming {}  {}  {}",
            short_id(&rec.session_id),
            record_location(&rec),
            display_title(&rec)
        );
    }
    Ok(plan_resume(&rec, args.extra))
}

fn candidates_phrase(n: usize) -> String {
    if n == 1 {
        "1 session matches".to_string()
    } else {
        format!("{n} sessions match")
    }
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
        ResumeAction::Cancelled => {
            eprintln!("{} cancelled", "✗".red());
            std::process::exit(EXIT_CANCELLED);
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
// cargo-mutants runs on unix, where this body is compiled out, so no test can ever catch a mutant.
#[mutants::skip]
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
