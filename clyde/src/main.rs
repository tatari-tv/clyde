#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `clyde` is the thin clap shim and composition root: it parses args, calls the `session` /
//! `sessions` libraries, and is the only crate that prints. All logic lives in the libs.

mod bootstrap;
mod cli;
mod doctor;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, FromArgMatches};
use colored::Colorize;
use eyre::{Context, Result};
use log::{LevelFilter, warn};

use cli::{
    Cli, Command, EnrichArgs, LsArgs, ReindexArgs, ResumeArgs, SearchArgs, ServeArgs, SessionsCommand, StageArgs,
    TagArgs,
};

/// Default log level for the clyde-native `sessions` subtree when `--log-level` is unset. The
/// absorbed tools keep their own defaults (via `Globals::log_level == None`), so this applies
/// only to the sessions arm.
const DEFAULT_LOG_LEVEL: &str = "info";
use sessions::{
    AnthropicClient, Db, EnrichOptions, EnrichStats, EnrichSummary, Filters, MatchSource, ReindexStats, SearchHit,
    ServeOpts, SessionRecord, StageStats,
};

/// Max width of a title in the human (terminal) listing before it is truncated with an ellipsis.
const TITLE_DISPLAY_WIDTH: usize = 80;

/// Indent for the second (metadata) line of a stacked session entry, so it reads as subordinate
/// to the full session ID on the line above.
const RECORD_INDENT: &str = "    ";

fn main() -> Result<()> {
    reset_sigpipe();
    let log_path = session::paths::data_root().join("logs").join("clyde.log");
    let after_help = format!("Logs are written to: {}", log_path.display());
    let mut command = Cli::command().after_help(after_help);
    // `clyde report --help` ends with report's REQUIRED TOOLS block (pandoc/marquee/git/jq
    // status). Building it spawns a `--version` probe per tool, so attach it only when the user
    // is actually asking for report's help — never on a normal `clyde report ...` invocation.
    let argv: Vec<String> = std::env::args().collect();
    if cli::report_help_requested(&argv) {
        command = command.mut_subcommand("report", |c| c.after_help(report::tool_validation_help()));
    }
    let cli = Cli::from_arg_matches(&command.get_matches())?;
    // The absorbed tools (report/cost/permit) own their own logging, output, and exit code, so
    // clyde must NOT install a logger for those arms — env_logger can only be initialized once
    // per process. Only the clyde-native `sessions` subtree sets up clyde's logger here.
    let level = cli.log_level.clone().unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string());
    if matches!(cli.command, Command::Report(_) | Command::Cost(_) | Command::Permit(_)) {
        // Absorbed tools install their own logger; clyde must not (one init per process).
    } else if is_serve(&cli) {
        // Serve mode keeps stdout reserved for JSON-RPC frames: rmcp/tokio emit via `tracing`, so
        // route logging through a file-target tracing subscriber (which also bridges `log`)
        // instead of env_logger.
        setup_serve_tracing(&level, &log_path)?;
    } else {
        // Every clyde-native arm (sessions non-serve, bootstrap, doctor) uses env_logger.
        setup_logging(&level, &log_path)?;
    }
    run(cli)
}

/// True when the parsed command is `clyde session serve` — the one arm that owns stdout for the
/// MCP protocol and therefore needs the file-target tracing subscriber.
fn is_serve(cli: &Cli) -> bool {
    matches!(
        cli.command,
        Command::Sessions {
            command: SessionsCommand::Serve(_)
        }
    )
}

/// Restore the default `SIGPIPE` disposition. Rust ignores SIGPIPE by default, which turns a
/// closed stdout (e.g. `clyde session search x | head`) into an EPIPE that `println!` unwraps
/// into a panic. Resetting to `SIG_DFL` makes clyde die quietly on a broken pipe like any Unix
/// filter. Done before any output is produced.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: single-threaded startup, before any I/O; the only effect is restoring the OS
    // default handler for SIGPIPE.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

/// Map an absorbed tool's `run() -> Result<i32>` onto a process exit code: a propagated error is
/// rendered to stderr and the process exits 1.
///
/// `debug` selects the rendering. At the default (info or lower verbosity) we print `{e:#}` — the
/// full eyre **cause chain** with NO `Location:`/backtrace — so a normal failure reads as a clean,
/// chained message instead of leaking an internal `report/src/config.rs:NNN` source location. Only
/// when `--log-level debug` (or trace) is set do we print `{e:?}` (Debug, with the location capture)
/// for diagnosis. Plain `{e}` is deliberately avoided: Display alone hides the causal chain and
/// would degrade the normal-failure UX.
fn dispatch_tool(result: Result<i32>, debug: bool) -> ! {
    let code = result.unwrap_or_else(|e| {
        if debug {
            eprintln!("{e:?}");
        } else {
            eprintln!("{e:#}");
        }
        1
    });
    std::process::exit(code);
}

/// True when the resolved log level is `debug` or `trace` — the verbosity at which the absorbed
/// tools' errors should render their full Debug form (with eyre's `Location:` capture) instead of
/// the clean cause chain. Unparseable levels are treated as non-debug.
fn is_debug_level(level: &str) -> bool {
    matches!(
        level.parse::<LevelFilter>(),
        Ok(LevelFilter::Debug | LevelFilter::Trace)
    )
}

fn run(cli: Cli) -> Result<()> {
    let globals = cli.globals();
    // Resolve the same level `main` used for logger setup, so the absorbed-tool error rendering
    // matches the verbosity the user asked for (clean cause chain by default, Debug+Location under
    // --log-level debug/trace).
    let debug = is_debug_level(cli.log_level.as_deref().unwrap_or(DEFAULT_LOG_LEVEL));
    let db_path = cli.db.clone().unwrap_or_else(session::paths::sessions_db_path);

    match cli.command {
        // Absorbed tools own their exit code and final printing; map it to process::exit, exactly
        // as their standalone shims do.
        Command::Report(args) => dispatch_tool(report::run(args, globals), debug),
        Command::Cost(args) => dispatch_tool(cost::run(args, globals), debug),
        Command::Permit(args) => dispatch_tool(permit::run(args, globals), debug),
        // clyde-native migration/health commands.
        Command::Bootstrap(args) => bootstrap::run(&args),
        Command::Doctor => std::process::exit(doctor::run()?),
        // Serve owns its own catalog handle (inside the async server) and needs a Tokio runtime,
        // so it is handled before opening the synchronous `Db` the other arms share.
        Command::Sessions {
            command: SessionsCommand::Serve(args),
        } => cmd_serve(&db_path, args),
        // Resume computes its action from the catalog, then drops the `Db` BEFORE acting: the
        // action is executed outside the `Db`'s scope so no SQLite handle is held across the exec
        // (rusqlite opens with `O_CLOEXEC`, so it would not actually leak, but dropping it first
        // makes that explicit and keeps the decision a pure, unit-testable function).
        Command::Sessions {
            command: SessionsCommand::Resume(args),
        } => {
            let action = {
                let db = Db::open_at(&db_path)?;
                cmd_resume(&db, args)?
                // `db` is dropped here, at the end of this block.
            };
            run_resume_action(action)
        }
        Command::Sessions { command } => {
            let db = Db::open_at(&db_path)?;
            match command {
                SessionsCommand::Search(args) => cmd_search(&db, args),
                // The bare-date `--since` convention is configurable via clyde.yml (default UTC).
                // Config is loaded lazily here, inside the commands that actually consume a tz, so
                // that a malformed clyde.yml never breaks commands that don't read dates
                // (search, resume, tag, reindex, doctor, bootstrap, serve).
                SessionsCommand::Ls(args) => {
                    let tz = load_date_tz()?;
                    cmd_ls(&db, args, tz)
                }
                SessionsCommand::Tag(args) => cmd_tag(&db, args),
                SessionsCommand::Reindex(args) => cmd_reindex(&db, args),
                SessionsCommand::Stage(args) => {
                    let tz = load_date_tz()?;
                    cmd_stage(&db, args, tz)
                }
                SessionsCommand::Enrich(args) => {
                    let tz = load_date_tz()?;
                    cmd_enrich(&db, args, tz)
                }
                SessionsCommand::Doctor => cmd_doctor(&db),
                // Unreachable: the outer arms above peel `Serve` and `Resume` off before this
                // shared `Db` block.
                SessionsCommand::Serve(_) => unreachable!("Serve is dispatched before opening Db"),
                SessionsCommand::Resume(_) => unreachable!("Resume is dispatched before this Db block"),
            }
        }
    }
}

/// Load `clyde.yml` and resolve the date-tz setting. Called lazily, only inside the subcommands
/// that parse a `--since`/`--dormant-after` date string (ls, stage, enrich). Commands that do not
/// consume date strings (search, resume, tag, reindex, doctor, bootstrap, serve) never call this,
/// so a malformed `clyde.yml` does not break them.
fn load_date_tz() -> Result<common::DateTz> {
    let config = common::config::load().context("failed to load clyde config")?;
    Ok(config.date_tz())
}

/// Bring up the MCP server on stdio. `clyde`'s `main`/`run` are synchronous, so the runtime is
/// built explicitly here and the only async work is the serve path; no other subcommand changes.
fn cmd_serve(db_path: &std::path::Path, args: ServeArgs) -> Result<()> {
    let projects_dir = args
        .projects_dir
        .or_else(session::paths::claude_projects_dir)
        .ok_or_else(|| eyre::eyre!("could not determine ~/.claude/projects (set HOME)"))?;
    let opts = ServeOpts {
        reindex_on_start: !args.no_reindex,
    };
    let runtime = tokio::runtime::Runtime::new().context("failed to build the serve Tokio runtime")?;
    runtime.block_on(sessions::serve_stdio(db_path, &projects_dir, opts))
}

fn cmd_search(db: &Db, args: SearchArgs) -> Result<()> {
    lazy_reindex(db, args.no_reindex);
    let query = args.query.join(" ");
    let hits = db.search(&query, args.limit, args.include_archived, args.sort.into())?;
    print_hits(&hits);
    Ok(())
}

fn cmd_ls(db: &Db, args: LsArgs, tz: common::DateTz) -> Result<()> {
    lazy_reindex(db, args.no_reindex);
    let since = match args.since.as_deref() {
        Some(s) => Some(sessions::parse_since(s, tz)?),
        None => None,
    };
    let filters = Filters {
        repo: args.repo,
        since,
        tag: args.tag,
        model: args.model,
        include_archived: args.include_archived,
        limit: args.limit,
    };
    let records = db.list(&filters)?;
    print_records(&records);
    Ok(())
}

/// What `resume` should do, computed from a resolved [`SessionRecord`] by [`plan_resume`]. Returned
/// to `main` (which drops the `Db` and then acts on it) so the decision stays a pure, unit-testable
/// function and no SQLite handle is held across the exec.
#[derive(Debug, PartialEq, Eq)]
enum ResumeAction {
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
fn plan_resume(rec: &SessionRecord, extra: Vec<String>) -> ResumeAction {
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
fn cmd_resume(db: &Db, args: ResumeArgs) -> Result<ResumeAction> {
    // `extra` is arbitrary passthrough forwarded verbatim to claude and may contain sensitive
    // values (tokens, endpoints); log only its length, never the contents.
    log::debug!(
        "cmd_resume: id={} no_reindex={} extra_count={}",
        args.id,
        args.no_reindex,
        args.extra.len()
    );
    lazy_reindex(db, args.no_reindex);
    let ids = db.resolve_id(&args.id)?;
    let id = match ids.as_slice() {
        [id] => id.clone(),
        [] => {
            eprintln!("{} no session matches {:?}", "✗".red(), args.id);
            std::process::exit(1);
        }
        many => {
            eprintln!("{} {:?} is ambiguous ({} matches)", "✗".red(), args.id, many.len());
            std::process::exit(1);
        }
    };
    let rec = db
        .get(&id)?
        .ok_or_else(|| eyre::eyre!("session {id} resolved but not found in the catalog"))?;
    Ok(plan_resume(&rec, args.extra))
}

/// Execute the planned [`ResumeAction`]. `Launch` replaces this process with `claude` (never returns
/// on unix success); every other variant prints a specific stderr message and exits non-zero.
fn run_resume_action(action: ResumeAction) -> Result<()> {
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

fn cmd_tag(db: &Db, args: TagArgs) -> Result<()> {
    let ids = db.resolve_id(&args.id)?;
    let id = match ids.as_slice() {
        [id] => id.clone(),
        [] => {
            eprintln!("{} no session matches {:?}", "✗".red(), args.id);
            std::process::exit(1);
        }
        many => {
            eprintln!("{} {:?} is ambiguous ({} matches)", "✗".red(), args.id, many.len());
            std::process::exit(1);
        }
    };
    if db.set_tags(&id, &args.tags)? {
        if args.tags.is_empty() {
            println!("{} cleared tags for {}", "✓".green(), short_id(&id));
        } else {
            println!("{} tagged {} with {}", "✓".green(), short_id(&id), args.tags.join(" "));
        }
    } else {
        eprintln!("{} session {id} not found", "✗".red());
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_reindex(db: &Db, args: ReindexArgs) -> Result<()> {
    let projects_dir = args
        .projects_dir
        .or_else(session::paths::claude_projects_dir)
        .ok_or_else(|| eyre::eyre!("could not determine ~/.claude/projects (set HOME)"))?;
    let stats = sessions::reindex(db, &projects_dir)?;
    print_reindex(&stats);
    Ok(())
}

fn cmd_stage(db: &Db, args: StageArgs, tz: common::DateTz) -> Result<()> {
    // Stage off fresh mtimes, so dormancy reflects the latest activity.
    lazy_reindex(db, false);
    let dormant_before = if args.all {
        None
    } else {
        Some(sessions::parse_since(&args.dormant_after, tz)?)
    };
    let staged_root = session::paths::staged_dir();
    let stats = sessions::stage_dormant(db, dormant_before, &staged_root)?;
    print_stage(&stats);
    Ok(())
}

fn cmd_enrich(db: &Db, args: EnrichArgs, tz: common::DateTz) -> Result<()> {
    // Enrich off fresh mtimes so dormancy and grown-since detection reflect the latest activity.
    lazy_reindex(db, false);
    if args.show_payload.is_some() && !args.dry_run {
        eyre::bail!("--show-payload only applies with --dry-run");
    }
    if args.id.is_some() && args.all {
        eyre::bail!("pass a session id OR --all, not both");
    }
    // --all and a single id ignore dormancy; the default sweep honors it.
    let dormant_before = if args.all || args.id.is_some() {
        None
    } else {
        Some(sessions::parse_since(&args.dormant_after, tz)?)
    };
    // Resolve a manual id/prefix to one concrete session (same fuzzy contract as resume/tag).
    let only = match &args.id {
        Some(needle) => match db.resolve_id(needle)?.as_slice() {
            [id] => Some(id.clone()),
            [] => {
                eprintln!("{} no session matches {:?}", "✗".red(), needle);
                std::process::exit(1);
            }
            many => {
                eprintln!("{} {:?} is ambiguous ({} matches)", "✗".red(), needle, many.len());
                std::process::exit(1);
            }
        },
        None => None,
    };
    let opts = EnrichOptions {
        dormant_before,
        all: args.all,
        only,
        dry_run: args.dry_run,
        show_payload: args.show_payload,
        max_attempts: args.max_attempts,
        token_budget: args.budget_tokens,
    };
    let stats = if args.dry_run {
        // No off-machine calls, no key needed: the gate is previewed, not opened.
        sessions::enrich::<AnthropicClient>(db, None, &opts)?
    } else {
        let client = AnthropicClient::from_env()?;
        sessions::enrich(db, Some(&client), &opts)?
    };
    print_enrich(&stats);
    Ok(())
}

fn cmd_doctor(db: &Db) -> Result<()> {
    let summary = db.enrich_summary()?;
    print_doctor(&summary);
    Ok(())
}

/// Refresh the catalog before a query (incremental, cheap). Failures warn but never abort the
/// query — stale data beats no answer.
fn lazy_reindex(db: &Db, skip: bool) {
    if skip {
        return;
    }
    let Some(projects_dir) = session::paths::claude_projects_dir() else {
        warn!("lazy_reindex: cannot resolve ~/.claude/projects; querying stored data only");
        return;
    };
    if let Err(e) = sessions::reindex(db, &projects_dir) {
        warn!("lazy_reindex: reindex failed, querying stored data only: {e}");
    }
}

fn print_hits(hits: &[SearchHit]) {
    if !std::io::stdout().is_terminal() {
        print_json(hits);
        return;
    }
    if hits.is_empty() {
        println!("{}", "no matching sessions".dimmed());
        return;
    }
    let msgs_width = msgs_column_width(hits.iter().map(|h| h.record.n_msgs));
    for hit in hits {
        let marker = match hit.matched {
            MatchSource::HighSignal => "●".green(),
            MatchSource::Body => "○".dimmed(),
        };
        print!("{marker} ");
        print_record_line(&hit.record, msgs_width);
    }
}

fn print_records(records: &[SessionRecord]) {
    if !std::io::stdout().is_terminal() {
        print_json(records);
        return;
    }
    if records.is_empty() {
        println!("{}", "no sessions".dimmed());
        return;
    }
    let msgs_width = msgs_column_width(records.iter().map(|r| r.n_msgs));
    for rec in records {
        print_record_line(rec, msgs_width);
    }
}

/// Single-line, width-capped title for terminal output. A title that fell back to a
/// (multi-line, possibly huge) first prompt is collapsed to one line and truncated; the JSON
/// surface keeps the full value.
fn display_title(rec: &SessionRecord) -> String {
    truncate_title(rec.title.as_deref().unwrap_or("(untitled)"))
}

/// Collapse whitespace to a single line and truncate to [`TITLE_DISPLAY_WIDTH`] chars (char-safe).
fn truncate_title(raw: &str) -> String {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > TITLE_DISPLAY_WIDTH {
        let kept: String = one_line.chars().take(TITLE_DISPLAY_WIDTH - 1).collect();
        format!("{kept}…")
    } else {
        one_line
    }
}

/// Two lines per session. First line: the full session ID (never truncated or wrapped, so it can
/// be copied verbatim into other clyde commands) followed by `<repo:branch>[tags] (archived)`.
/// Second line, indented: `<date>  <n>  <title>`. Stacking keeps the un-truncatable ID off the
/// width budget so narrow terminals don't wrap it.
/// `msgs_width` right-justifies the message count so the title column lines up across rows.
fn print_record_line(rec: &SessionRecord, msgs_width: usize) {
    let title = display_title(rec);
    let repo = rec.cwd.as_deref().and_then(|c| c.rsplit('/').next()).unwrap_or("-");
    // A detached HEAD (or a cwd outside any repo) is recorded as the literal "HEAD" by Claude
    // Code; that's not a meaningful branch name, so render it as an empty branch (`repo:`).
    let branch = match rec.git_branch.as_deref() {
        None | Some("HEAD") => "",
        Some(b) => b,
    };
    let date = rec.modified.format("%Y-%m-%d");
    let tags = if rec.tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", rec.tags.join(" "))
    };
    let archived = if rec.archived { " (archived)".red().to_string() } else { String::new() };
    // Drop the `:` separator when there's no branch (non-repo cwd or detached HEAD) so the
    // location reads as a plain `repo` instead of a dangling `repo:`.
    let location = if branch.is_empty() { repo.to_string() } else { format!("{repo}:{branch}") };
    println!(
        "{}  {}{}{}",
        rec.session_id.yellow(),
        location.cyan(),
        tags.green(),
        archived,
    );
    println!(
        "{RECORD_INDENT}{} {:>width$} {}",
        date.to_string().dimmed(),
        rec.n_msgs,
        title.as_str().bold(),
        width = msgs_width,
    );
}

/// Width of the widest message count across `counts`, for right-justified column alignment.
fn msgs_column_width(counts: impl Iterator<Item = i64>) -> usize {
    counts.map(|n| n.to_string().len()).max().unwrap_or(1)
}

fn print_reindex(stats: &ReindexStats) {
    if std::io::stdout().is_terminal() {
        println!(
            "{} scanned {}, upserted {}, skipped {}, archived {}",
            "✓".green(),
            stats.scanned,
            stats.upserted,
            stats.skipped_unchanged,
            stats.archived,
        );
    } else {
        print_json(stats);
    }
}

fn print_stage(stats: &StageStats) {
    if std::io::stdout().is_terminal() {
        println!(
            "{} considered {}, staged {}, up-to-date {} ({} files copied)",
            "✓".green(),
            stats.considered,
            stats.staged,
            stats.up_to_date,
            stats.files_copied,
        );
    } else {
        print_json(stats);
    }
}

fn print_enrich(stats: &EnrichStats) {
    if !std::io::stdout().is_terminal() {
        print_json(stats);
        return;
    }
    // Dry-run: show the per-session gate decisions the operator is previewing.
    if stats.dry_run {
        for d in &stats.details {
            let send = if d.would_send { "send".green() } else { "skip".dimmed() };
            let metrics = match (d.redaction_count, d.payload_bytes) {
                (Some(r), Some(b)) => format!("{r} redactions, {b}B"),
                _ => "-".to_string(),
            };
            println!(
                "{}  {}  {}  {}  {}",
                short_id(&d.session_id).yellow(),
                d.scope.as_str().cyan(),
                send,
                metrics.dimmed(),
                d.status.dimmed(),
            );
        }
    }
    let verb = if stats.dry_run { "would enrich" } else { "enriched" };
    let n = if stats.dry_run { stats.would_enrich } else { stats.enriched };
    println!(
        "{} considered {}, {} {}, skipped-personal {}, skipped-empty {}, failed {} ({} redactions, {} tokens in / {} out)",
        "✓".green(),
        stats.considered,
        verb,
        n,
        stats.skipped_personal,
        stats.skipped_empty,
        stats.failed,
        stats.redactions,
        stats.tokens_in,
        stats.tokens_out,
    );
}

fn print_doctor(summary: &EnrichSummary) {
    if !std::io::stdout().is_terminal() {
        print_json(summary);
        return;
    }
    let last = summary
        .last_enriched_at
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "never".to_string());
    println!(
        "{} {} sessions: enriched {}, never-enriched {}, skipped-personal {}, skipped-empty {}, failed {}",
        "✓".green(),
        summary.total,
        summary.enriched,
        summary.never_enriched,
        summary.skipped_personal,
        summary.skipped_empty,
        summary.failed,
    );
    println!("  last successful enrichment: {}", last.dimmed());
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("{} failed to serialize output: {e}", "✗".red()),
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests;

fn setup_logging(level: &str, log_path: &PathBuf) -> Result<()> {
    let level: LevelFilter = level.parse().unwrap_or(LevelFilter::Info);
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create log dir {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open log file {}", log_path.display()))?;
    env_logger::Builder::new()
        .filter_level(level)
        .target(env_logger::Target::Pipe(Box::new(file)))
        .init();
    Ok(())
}

/// Serve-mode logging: a file-target `tracing` subscriber, NOT env_logger. rmcp and tokio emit
/// via `tracing` (not `log`), and stdout is reserved for JSON-RPC framing, so their output must
/// land in the log file. `tracing_subscriber::fmt().init()` also installs the `tracing-log`
/// bridge, so clyde's own `log::*` records (e.g. reindex warnings) are captured by the same
/// subscriber. Mutually exclusive with [`setup_logging`]: installing both would race for the
/// global `log` logger slot and panic.
fn setup_serve_tracing(level: &str, log_path: &PathBuf) -> Result<()> {
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create log dir {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open log file {}", log_path.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(level))
        .with_writer(file)
        .with_ansi(false)
        .init();
    Ok(())
}
