#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod aggregate;
pub mod cli;
pub mod config;
pub mod fmt;
pub mod merge;
pub mod outcome;
pub mod persona;
pub mod render;
pub mod repo;
pub mod report;
pub mod summarize;
pub mod title;
pub mod tools;

use crate::config::{CollectConfig, Output};
use claude_pricing::Pricing;
use efficiency::{Outcomes, SessionEfficiency};
use eyre::{Context, Result};
use log::{LevelFilter, debug};
use sessions::{CatalogEntry, Db, Filters};
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub use cli::ReportArgs;
pub use config::{Config, ResolvedCommand};
pub use tools::tool_validation_help;

#[derive(Debug)]
pub struct RunResult {
    pub sessions_emitted: usize,
    /// Where the output went. For a streamed collect (`-o` omitted) this is
    /// [`OutputDest::Stdout`]; otherwise the concrete file path.
    pub output: OutputDest,
}

/// Human-facing description of where a run's output landed, for the post-run "wrote N" message.
#[derive(Debug, Clone)]
pub enum OutputDest {
    File(PathBuf),
    Stdout,
    /// A published marquee post; carries the resulting URL (from `--format marquee-*`).
    Marquee(String),
}

impl std::fmt::Display for OutputDest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputDest::File(p) => write!(f, "{}", p.display()),
            OutputDest::Stdout => write!(f, "stdout"),
            OutputDest::Marquee(url) => write!(f, "{}", url),
        }
    }
}

/// Behavior-exact entry point for both the `cr` shim and `clyde report`. Owns logging setup,
/// the `merge`-unimplemented exit-2 path, the `collect`-needs-jq exit-2 preflight, the final
/// success print, and the process exit code. Returns the intended exit code; the caller maps it
/// to `process::exit`.
pub fn run(args: ReportArgs, globals: common::Globals) -> Result<i32> {
    let log_level = globals.log_level.unwrap_or_else(|| "info".to_string());
    setup_logging(&log_level).context("Failed to setup logging")?;

    let config = Config {
        // clyde.yml (the bare-date `--since` tz convention) is loaded LAZILY inside
        // `resolve_command` — only for `collect`, the sole DateTz consumer — so a malformed
        // config cannot break `report render`/`merge`, which never use a date.
        command: config::resolve_command(args.command)?,
        log_level,
    };

    if let ResolvedCommand::Collect(_) = config.command
        && which::which("jq").is_err()
    {
        // Advisory, NON-FATAL: collect produces its JSON regardless — `jq` is never used
        // internally, only by the user to query the output. Don't refuse to run (that broke
        // `collect` on any host/CI without jq). The note goes to stderr so it can't corrupt the
        // JSON streamed to stdout.
        eprintln!(
            "note: jq not found on PATH; the JSON report is still produced. Install jq to query it \
             (brew install jq | apt install jq | dnf install jq)."
        );
    }

    let result = run_with_config(&config).context("report failed")?;
    // HAZARD 1 (review-flagged): the status line MUST go to stderr, not stdout. When collect
    // streams its JSON to stdout (`-o` omitted), a `println!` here would interleave on stdout and
    // corrupt the JSON stream that `... | jq` consumes.
    eprintln!("wrote {} sessions to {}", result.sessions_emitted, result.output);
    // A published marquee post's whole value is a shareable URL, so ALSO emit the bare URL to
    // stdout — that is the machine-readable result (`url=$(clyde report render --format ...)`),
    // matching the collect convention of "payload on stdout, status on stderr". Other destinations
    // (file/stdout-markdown) have no separate machine result: the file path is in the status line
    // and markdown was already written to stdout by render.
    if let OutputDest::Marquee(url) = &result.output {
        println!("{url}");
    }
    Ok(0)
}

/// Path to report's log file, unified under `<xdg-data>/clyde/logs/report.log` (Phase 8, D3: log
/// paths are declared outside the behavior-exact shim surface). `pub` so the caller renders the
/// same dynamic path the logger actually writes.
pub fn log_file_path() -> PathBuf {
    crate::config::xdg_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clyde")
        .join("logs")
        .join("report.log")
}

/// File-target logger to the unified `clyde/logs/report.log` path (Phase 8).
fn setup_logging(level: &str) -> Result<()> {
    let log_file = log_file_path();
    let log_dir = log_file.parent().expect("log file has parent");
    std::fs::create_dir_all(log_dir).context("Failed to create log directory")?;
    let target = Box::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );
    let filter = LevelFilter::from_str(level).unwrap_or(LevelFilter::Info);
    env_logger::Builder::new()
        .filter_level(filter)
        .target(env_logger::Target::Pipe(target))
        .init();
    Ok(())
}

pub fn run_with_config(config: &Config) -> Result<RunResult> {
    let pricing = Pricing::auto("clyde").context("failed to load pricing")?;
    run_with_pricing(config, &pricing)
}

pub(crate) fn run_with_pricing(config: &Config, pricing: &Pricing) -> Result<RunResult> {
    match &config.command {
        ResolvedCommand::Collect(cfg) => run_collect(cfg, pricing),
        ResolvedCommand::Render(cfg) => render::run(cfg, pricing),
        ResolvedCommand::Merge(cfg) => merge::run(cfg),
    }
}

/// Collect-once, from the canonical catalog. Reads the `[since, until]` window from `sessions.db`
/// (session rows + the RAW `efficiency_json` / `outcome_json` blobs, one query), parses the blobs
/// with `efficiency`'s types, and shapes a schema-v2 report. NO JSONL is scanned — tokens, cost,
/// cache/tool/agent-type signals and outcomes are all catalog-sourced.
///
/// Fails closed on an incomplete catalog: if any session in the window has a NULL `efficiency_json`
/// (not yet reindexed) collect writes a `clyde session reindex` remedy to STDERR, emits NO artifact,
/// does not overwrite the target, and exits non-zero. An empty window is a VALID empty report
/// (exit 0); an unparseable blob is a LOUD error (bad data ≠ no data).
fn run_collect(cfg: &CollectConfig, pricing: &Pricing) -> Result<RunResult> {
    debug!(
        "run_collect: db_path={} since={} until={} no_rollup={} no_outcomes={}",
        cfg.db_path.display(),
        cfg.since,
        cfg.until,
        cfg.no_rollup,
        cfg.no_outcomes
    );

    let db = Db::open_at(&cfg.db_path)
        .with_context(|| format!("failed to open the session catalog at {}", cfg.db_path.display()))?;
    let filters = Filters {
        since: Some(cfg.since),
        until: Some(cfg.until),
        include_archived: false,
        ..Default::default()
    };
    let entries = db
        .catalog(&filters)
        .context("failed to read the session catalog window")?;
    log::info!("run_collect: catalog returned {} sessions in the window", entries.len());

    // Fail closed: any windowed session not yet reindexed (NULL efficiency_json) means the catalog
    // is incomplete. Never zero-fill or emit a partial artifact; point the operator at the remedy.
    let missing = entries.iter().filter(|e| e.efficiency_json.is_none()).count();
    if missing > 0 {
        // Remedy to STDERR (status channel) so a piped stdout JSON stream is never poisoned; NO
        // artifact is written and the target file is left untouched.
        eprintln!(
            "error: {missing} session(s) in the window have no efficiency data in the catalog (not \
             yet indexed). Run `clyde session reindex` to backfill them, then re-run `report \
             collect`. No report was written."
        );
        return Err(eyre::eyre!(
            "incomplete catalog: {missing} session(s) missing efficiency data; run `clyde session reindex`"
        ));
    }

    let outcomes_enabled = !cfg.no_outcomes;
    let mut resolver = repo::Resolver::new();
    let mut collected: Vec<report::CollectedSession> = Vec::with_capacity(entries.len());
    for entry in &entries {
        collected.push(to_collected(entry, outcomes_enabled, &mut resolver)?);
    }

    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let (count, dest) = match &cfg.output {
        Output::File(path) => {
            let count = report::write_json(
                path,
                &collected,
                cfg.since,
                cfg.until,
                &host,
                pricing,
                outcomes_enabled,
                cfg.no_rollup,
            )?;
            (count, OutputDest::File(path.clone()))
        }
        Output::Stdout => {
            let (json, count) = report::build_json(
                &collected,
                cfg.since,
                cfg.until,
                &host,
                pricing,
                outcomes_enabled,
                cfg.no_rollup,
            )?;
            // Stream the JSON to stdout (and only the JSON — the "wrote N" note is on stderr).
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            out.write_all(json.as_bytes())
                .context("failed to write report JSON to stdout")?;
            out.write_all(b"\n")
                .context("failed to write trailing newline to stdout")?;
            (count, OutputDest::Stdout)
        }
    };

    Ok(RunResult {
        sessions_emitted: count,
        output: dest,
    })
}

/// Parse one catalog row into a [`report::CollectedSession`]. `efficiency_json` is guaranteed
/// present here (the NULL fail-closed guard already returned), so a NULL past this point is an
/// internal invariant break, and an unparseable blob is a LOUD error (bad data ≠ no data). Titles,
/// repo (from `cwd`), and the window timestamps all come from the catalog row — no JSONL is read.
fn to_collected(
    entry: &CatalogEntry,
    outcomes_enabled: bool,
    resolver: &mut repo::Resolver,
) -> Result<report::CollectedSession> {
    let rec = &entry.record;
    let json = entry.efficiency_json.as_deref().ok_or_else(|| {
        eyre::eyre!(
            "internal: NULL efficiency_json past the fail-closed guard for session {}",
            rec.session_id
        )
    })?;
    let efficiency: SessionEfficiency = serde_json::from_str(json)
        .with_context(|| format!("failed to parse efficiency_json for session {}", rec.session_id))?;

    let outcomes = if outcomes_enabled {
        match entry.outcome_json.as_deref() {
            Some(oj) => {
                let parsed: Outcomes = serde_json::from_str(oj)
                    .with_context(|| format!("failed to parse outcome_json for session {}", rec.session_id))?;
                // A stored all-empty object means "reindexed, no outcomes"; collapse it to `None` so
                // the artifact carries an `outcomes` field only for sessions that produced something.
                if parsed == Outcomes::default() {
                    None
                } else {
                    Some(parsed)
                }
            }
            None => None,
        }
    } else {
        None
    };

    let repo = rec.cwd.as_deref().and_then(|c| resolver.detect(Path::new(c)));
    // Session-level window (M2): begin = the session's created time (fallback modified), end = its
    // modified time. These bound the artifact's day/outlier attribution, not a per-record scan.
    let begin = rec.created.unwrap_or(rec.modified);
    let end = rec.modified;

    Ok(report::CollectedSession {
        session_id: rec.session_id.clone(),
        title: rec.title.clone(),
        repo,
        begin,
        end,
        jsonl_paths: vec![rec.transcript_path.clone()],
        efficiency,
        outcomes,
    })
}

#[cfg(test)]
mod tests;
