//! Clap definitions for `clyde`. Parsing only; validation/dispatch lives in `main.rs`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "clyde",
    about = "Catalog, search, and resume Claude Code sessions",
    version = env!("GIT_DESCRIBE"),
    arg_required_else_help = true,
)]
pub struct Cli {
    /// Log level (error, warn, info, debug, trace). The single common global; clyde passes it
    /// down to each absorbed tool. When unset, the `sessions` subtree defaults to `info` and the
    /// absorbed tools fall back to their own prior defaults.
    #[arg(short = 'l', long, global = true)]
    pub log_level: Option<String>,

    /// Override the sessions.db path (default: $XDG_DATA_HOME/clyde/sessions.db).
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// The common globals clyde passes down to each absorbed tool's `run(args, globals)`.
    pub fn globals(&self) -> common::Globals {
        common::Globals {
            log_level: self.log_level.clone(),
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Catalog and search Claude Code sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// Scan Claude Code session JSONL files and emit a per-host JSON report (was `cr`).
    Report(report::ReportArgs),
    /// Track Claude Code session costs and usage; install the statusline (was `ccu`).
    Cost(cost::CostArgs),
    /// Manage Claude Code permission hygiene; the PreToolUse hook entry (was `claude-permit`).
    Permit(permit::PermitArgs),
    /// Migrate config/data/cache under one clyde home and repoint the live integrations.
    Bootstrap(crate::bootstrap::BootstrapArgs),
    /// Health-check the migration and integrations; non-zero exit while any legacy target remains.
    Doctor,
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
    /// Serve the session catalog over MCP (stdio). Intended to be spawned by an MCP host
    /// (e.g. Claude Code), not run by hand: it speaks JSON-RPC on stdin/stdout.
    Serve(ServeArgs),
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

#[derive(clap::Args, Debug)]
pub struct ServeArgs {
    /// Override the Claude projects dir (default: ~/.claude/projects).
    #[arg(long)]
    pub projects_dir: Option<PathBuf>,
    /// Skip the one-time reindex at startup (serve a possibly-stale catalog).
    #[arg(long)]
    pub no_reindex: bool,
}

#[cfg(test)]
mod tests;
