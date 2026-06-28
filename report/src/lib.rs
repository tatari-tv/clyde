#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod cli;
pub mod config;
pub mod merge;
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

pub use cli::{ReportArgs, ReportCli};
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
}

impl std::fmt::Display for OutputDest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputDest::File(p) => write!(f, "{}", p.display()),
            OutputDest::Stdout => write!(f, "stdout"),
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

    // The bare-date `--since` midnight convention is configurable via clyde.yml (default UTC),
    // shared with the sessions tool through `common`. A missing config is not an error.
    let tz = common::config::load().context("failed to load clyde config")?.date_tz();
    let config = Config {
        command: config::resolve_command(args.command, tz)?,
        log_level,
    };

    if let ResolvedCommand::Collect(_) = config.command
        && which::which("jq").is_err()
    {
        eprintln!(
            "jq is required to query the JSON report output but was not found on PATH.\n\
             Install: brew install jq  (macOS) | apt install jq  (Debian/Ubuntu) | dnf install jq  (Fedora)"
        );
        return Ok(2);
    }

    let result = run_with_config(&config).context("report failed")?;
    // HAZARD 1 (review-flagged): this MUST go to stderr, not stdout. When collect streams its
    // JSON to stdout (`-o` omitted), a `println!` here would interleave on stdout and corrupt
    // the JSON stream that `... | jq` consumes.
    eprintln!("wrote {} sessions to {}", result.sessions_emitted, result.output);
    Ok(0)
}

/// File-target logger to `~/.local/share/claude-report/logs/claude-report.log`. Preserved exactly
/// from the pre-merge `cr` binary so the tool's log destination is unchanged.
fn setup_logging(level: &str) -> Result<()> {
    let log_dir = crate::config::xdg_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-report")
        .join("logs");
    std::fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    let log_file = log_dir.join("claude-report.log");
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
        ResolvedCommand::Render(cfg) => render::run(cfg),
        ResolvedCommand::Merge(cfg) => merge::run(cfg),
    }
}

fn run_collect(cfg: &CollectConfig, pricing: &Pricing) -> Result<RunResult> {
    let files = scan::find_session_files(&cfg.projects_dir)?;
    log::info!("run_collect: discovered {} session files", files.len());

    let parsed: HashMap<PathBuf, ParseResult> = files
        .par_iter()
        .filter_map(|f| match parse_jsonl_file(&f.path) {
            Ok(r) => Some((f.path.clone(), r)),
            Err(e) => {
                log::warn!("parse failed for {}: {}", f.path.display(), e);
                None
            }
        })
        .collect();

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
        cfg.since,
        cfg.until,
        cfg.no_rollup,
        &mut resolver,
        &existing_titles,
    );

    if !cfg.skip_title {
        title_untitled_sessions(&mut summaries);
    }

    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let (count, dest) = match &cfg.output {
        Output::File(path) => {
            let count = report::write_json(path, &summaries, cfg.since, cfg.until, &host, pricing)?;
            (count, OutputDest::File(path.clone()))
        }
        Output::Stdout => {
            let (json, count) = report::build_json(&summaries, cfg.since, cfg.until, &host, pricing)?;
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
