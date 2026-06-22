//! Clap definitions for `klod`. Parsing only; validation/dispatch lives in `main.rs`.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand};
use eyre::{Result, bail};

#[derive(Parser, Debug)]
#[command(
    name = "klod",
    about = "Catalog, search, and resume Claude Code sessions",
    version = env!("GIT_DESCRIBE"),
    arg_required_else_help = true,
)]
pub struct Cli {
    /// Log level (error, warn, info, debug, trace).
    #[arg(short = 'l', long, global = true, default_value = "info")]
    pub log_level: String,

    /// Override the sessions.db path (default: $XDG_DATA_HOME/klod/sessions.db).
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Catalog and search Claude Code sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// Full-text search over sessions, ranked (high-signal fields first).
    Search(SearchArgs),
    /// List sessions filtered by repo / date / tag / model.
    Ls(LsArgs),
    /// Print the `claude --resume <uuid>` line for a session (by id or unique prefix).
    Open(OpenArgs),
    /// Set tags on a session (space-separated; replaces existing tags).
    Tag(TagArgs),
    /// Reindex the catalog from ~/.claude/projects (incremental, mtime-skip).
    Reindex(ReindexArgs),
    /// Stage durable copies of dormant transcripts before the 30-day TTL reaps them.
    Stage(StageArgs),
    /// Enrich dormant sessions: fill tags + summary via a Haiku pass (work-scoped only).
    Enrich(EnrichArgs),
    /// Report enrichment health: counts and the last successful enrichment.
    Doctor,
}

#[derive(clap::Args, Debug)]
pub struct SearchArgs {
    /// Search terms (space-separated; all terms must match).
    #[arg(required = true, num_args = 1..)]
    pub query: Vec<String>,
    /// Cap on results.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Include TTL-reaped (archived) sessions.
    #[arg(long)]
    pub include_archived: bool,
    /// Skip the lazy reindex that normally refreshes the catalog before querying.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(clap::Args, Debug)]
pub struct LsArgs {
    /// Substring match against cwd / project (e.g. a repo name).
    #[arg(long)]
    pub repo: Option<String>,
    /// Only sessions modified since this point: a relative span (e.g. 7d, 24h, 30m) or a date.
    #[arg(long)]
    pub since: Option<String>,
    /// Require this tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Substring match against the model id.
    #[arg(long)]
    pub model: Option<String>,
    /// Cap on rows.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Include TTL-reaped (archived) sessions.
    #[arg(long)]
    pub include_archived: bool,
    /// Skip the lazy reindex that normally refreshes the catalog before querying.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(clap::Args, Debug)]
pub struct OpenArgs {
    /// Session id or a unique prefix of it.
    pub id: String,
    /// Skip the lazy reindex before resolving.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(clap::Args, Debug)]
pub struct TagArgs {
    /// Session id or a unique prefix of it.
    pub id: String,
    /// Tags to set (space-separated; replaces any existing tags).
    #[arg(required = true, num_args = 1..)]
    pub tags: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct ReindexArgs {
    /// Override the Claude projects dir (default: ~/.claude/projects).
    #[arg(long)]
    pub projects_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct StageArgs {
    /// Treat a session as dormant once it has been idle this long (e.g. 7d, 24h). Default 7d.
    #[arg(long, default_value = "7d")]
    pub dormant_after: String,
    /// Stage every non-archived session regardless of dormancy.
    #[arg(long)]
    pub all: bool,
}

#[derive(clap::Args, Debug)]
pub struct EnrichArgs {
    /// Enrich exactly one session by id or unique prefix (manual; bypasses dormancy + eligibility,
    /// and overrides manual-tag preservation).
    pub id: Option<String>,
    /// Re-enrich every eligible session (vocabulary refresh; overwrites manual tags).
    #[arg(long)]
    pub all: bool,
    /// Treat a session as dormant once it has been idle this long (e.g. 7d, 24h). Mirrors `stage`.
    #[arg(long, default_value = "7d")]
    pub dormant_after: String,
    /// Preview the gate's decisions (scope, would-send, redaction count, payload size) without
    /// sending anything off-machine.
    #[arg(long)]
    pub dry_run: bool,
    /// Dry-run only: write each redacted payload under this directory for the operator to inspect.
    #[arg(long)]
    pub show_payload: Option<PathBuf>,
    /// Per-session attempt cap before a repeatedly-failing session stops being retried.
    #[arg(long, default_value_t = sessions::enrich::DEFAULT_MAX_ATTEMPTS)]
    pub max_attempts: i64,
    /// Halt the sweep once cumulative tokens (in + out) reach this budget.
    #[arg(long)]
    pub budget_tokens: Option<u64>,
}

/// Parse a `--since` value: a relative span like `7d`/`24h`/`90m`/`30s`/`2w`, an RFC 3339
/// timestamp, or a `YYYY-MM-DD` date (interpreted as UTC midnight).
pub fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Some(dt) = parse_relative(s) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        && let Some(naive) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
    }
    bail!("could not parse --since '{s}': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD");
}

fn parse_relative(s: &str) -> Option<DateTime<Utc>> {
    let (num, unit) = s.split_at(s.char_indices().take_while(|(_, c)| c.is_ascii_digit()).count());
    if num.is_empty() {
        return None;
    }
    let n: i64 = num.parse().ok()?;
    let span = match unit {
        "s" => Duration::try_seconds(n),
        "m" => Duration::try_minutes(n),
        "h" => Duration::try_hours(n),
        "d" => Duration::try_days(n),
        "w" => Duration::try_weeks(n),
        _ => return None,
    }?;
    Some(Utc::now() - span)
}

#[cfg(test)]
mod tests;
