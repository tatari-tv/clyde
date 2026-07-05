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
pub mod scan;
pub mod session;
pub mod summarize;
pub mod title;

use crate::config::{CollectConfig, Output};
use claude_pricing::{ParseResult, Pricing, parse_jsonl_file};
use eyre::{Context, Result};
use log::LevelFilter;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

pub use cli::ReportArgs;
pub use config::{Config, ResolvedCommand};

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

fn run_collect(cfg: &CollectConfig, pricing: &Pricing) -> Result<RunResult> {
    let files = scan::find_session_files(&cfg.projects_dir)?;
    log::info!(
        "run_collect: discovered {} session files no-outcomes={}",
        files.len(),
        cfg.no_outcomes
    );

    // Each file is read twice inside the SAME par_iter closure while it is page-cache-hot: once
    // for pricing/usage (`parse_jsonl_file`) and once for outcome signals (`outcome::extract`).
    // Outcome extraction receives the period bounds so records are filtered by their initiating
    // timestamp at extraction time (D8). `--no-outcomes` (Phase 5) skips the second read
    // entirely rather than extracting and discarding: it is the documented escape hatch from
    // the design's performance section.
    let scanned: Vec<(PathBuf, ParseResult, outcome::FileOutcomes)> = files
        .par_iter()
        .filter_map(|f| {
            let parsed = match parse_jsonl_file(&f.path) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("parse failed for {}: {}", f.path.display(), e);
                    return None;
                }
            };
            let outcomes = if cfg.no_outcomes {
                outcome::FileOutcomes::default()
            } else {
                match outcome::extract(&f.path, cfg.since, cfg.until) {
                    Ok(o) => o,
                    Err(e) => {
                        // Fail closed: outcomes absent for this file, usage still counts.
                        log::warn!(
                            "outcome extraction failed for {}: {} (outcomes absent for this file)",
                            f.path.display(),
                            e
                        );
                        outcome::FileOutcomes::default()
                    }
                }
            };
            Some((f.path.clone(), parsed, outcomes))
        })
        .collect();

    let mut parsed: HashMap<PathBuf, ParseResult> = HashMap::with_capacity(scanned.len());
    let mut outcomes: HashMap<PathBuf, outcome::FileOutcomes> = HashMap::with_capacity(scanned.len());
    for (path, pr, fo) in scanned {
        parsed.insert(path.clone(), pr);
        outcomes.insert(path, fo);
    }

    // Titles are cached across runs in the prior report file. HAZARD 2 (review-flagged,
    // financial): the title cache MUST still be seeded when streaming to stdout (no `-o`), or
    // every run re-bills the paid Haiku titling because no prior titles carry forward. So we
    // always resolve a title-cache *source* directory (`Output::title_cache_dir`): for a file
    // target it is the output's parent; for stdout it is the default report dir under XDG data.
    // We seed from the newest prior `claude-report-*.json` in that directory (or the file
    // itself, if it already exists).
    let titles_source = resolve_titles_source(&cfg.output)?;
    let existing_titles = titles_source
        .as_deref()
        .map(report::load_existing_titles)
        .unwrap_or_default();
    let mut resolver = repo::Resolver::new();

    let mut summaries = session::fold(
        &files,
        &parsed,
        &outcomes,
        cfg.since,
        cfg.until,
        cfg.no_rollup,
        &mut resolver,
        &existing_titles,
    );

    if !cfg.skip_title {
        title_untitled_sessions(&mut summaries);
    }

    let outcomes_enabled = !cfg.no_outcomes;
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let (count, dest) = match &cfg.output {
        Output::File(path) => {
            let count = report::write_json(path, &summaries, cfg.since, cfg.until, &host, pricing, outcomes_enabled)?;
            (count, OutputDest::File(path.clone()))
        }
        Output::Stdout => {
            let (json, count) = report::build_json(&summaries, cfg.since, cfg.until, &host, pricing, outcomes_enabled)?;
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

/// Resolve the prior report to seed the cross-run title cache from. HAZARD 2: this is resolved
/// in BOTH file and stdout modes so the paid Haiku titling carries forward and does not re-bill
/// the Anthropic API on every run. For a file target that already exists, that file is the
/// source; otherwise (including all stdout runs) we scan the title-cache directory for the
/// newest prior `claude-report-*.json`.
fn resolve_titles_source(output: &Output) -> Result<Option<PathBuf>> {
    if let Output::File(path) = output
        && path.exists()
    {
        return Ok(Some(path.clone()));
    }
    let dir = output.title_cache_dir()?;
    Ok(latest_prior_report_in(&dir, output))
}

fn title_untitled_sessions(summaries: &mut [session::SessionSummary]) {
    let api_key = match title::api_key_from_env() {
        Some(k) => k,
        None => {
            log::info!("run_collect: ANTHROPIC_API_KEY not set; skipping titling");
            return;
        }
    };

    let to_title: Vec<usize> = summaries
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if s.title.is_none() && parent_jsonl(s).is_some() {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    if to_title.is_empty() {
        return;
    }

    log::info!("run_collect: titling {} sessions via Haiku", to_title.len());
    let titles: Vec<(usize, Option<String>)> = to_title
        .par_iter()
        .map(|&i| {
            let s = &summaries[i];
            let parent = match parent_jsonl(s) {
                Some(p) => p,
                None => return (i, None),
            };
            let prefix = match title::extract_prefix(parent) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("title: extract_prefix failed for {}: {}", parent.display(), e);
                    return (i, None);
                }
            };
            match title::haiku(&prefix, &api_key) {
                Ok(t) => (i, t),
                Err(e) => {
                    log::warn!("title::haiku failed for session {}: {}", s.session_id, e);
                    (i, None)
                }
            }
        })
        .collect();

    for (i, t) in titles {
        summaries[i].title = t;
    }
}

/// Find the most recent prior `claude-report-*.json` in `dir`, excluding the current output
/// file (if `output` is a concrete file in that dir). The `%Y-%m-%d-%H%M%S` stamp is lexically
/// ordered, so the greatest filename is the newest report — used to carry cached titles forward
/// across runs (including stdout runs, which scan the default report dir).
fn latest_prior_report_in(dir: &std::path::Path, output: &Output) -> Option<PathBuf> {
    let current = match output {
        Output::File(p) => p.file_name(),
        Output::Stdout => None,
    };
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name() != current
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("claude-report-") && n.ends_with(".json"))
        })
        .collect();
    candidates.sort();
    candidates.pop()
}

fn parent_jsonl(s: &session::SessionSummary) -> Option<&std::path::Path> {
    s.jsonl_paths
        .iter()
        .find(|p| !p.components().any(|c| c.as_os_str() == "subagents"))
        .map(|p| p.as_path())
}

#[cfg(test)]
mod tests;
