//! Clap definitions for `clyde`. Parsing only; validation/dispatch lives in `main.rs`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Result ordering for `clyde session search`. Maps to the domain [`sessions::SortBy`] enum via
/// `From<SortOrder>`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
#[clap(rename_all = "kebab-case")]
pub enum SortOrder {
    /// BM25 score primary, recency (modified DESC) as tiebreak. High-signal hits remain tiered
    /// above body hits.
    #[default]
    Relevance,
    /// modified DESC primary, BM25 score as tiebreak. Tiering is dissolved: the merged set is
    /// ordered globally by recency.
    Recency,
}

impl From<SortOrder> for sessions::SortBy {
    fn from(s: SortOrder) -> Self {
        match s {
            SortOrder::Relevance => sessions::SortBy::Relevance,
            SortOrder::Recency => sessions::SortBy::Recency,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "clyde",
    about = "Catalog, search, and resume Claude Code sessions",
    version = env!("GIT_DESCRIBE"),
    arg_required_else_help = true,
)]
pub struct Cli {
    /// Log level (error, warn, info, debug, trace). The single common global; clyde passes it
    /// down to each absorbed tool. When unset, the `session` subtree defaults to `info` and the
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
    /// Catalog, search, and resume sessions.
    #[command(name = "session")]
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// Per-host JSON usage report (was `cr`).
    Report(report::ReportArgs),
    /// Cost and usage tracking; statusline (was `ccu`).
    Cost(cost::CostArgs),
    /// Permission-hygiene hook and audit (was `claude-permit`).
    Permit(permit::PermitArgs),
    /// Migrate config/data/cache into one clyde home.
    Bootstrap(crate::bootstrap::BootstrapArgs),
    /// Health-check the migration and integrations.
    Doctor,
    /// Check for, install, or revert a newer released version of clyde.
    Update(renew::UpdateCmd),
    /// Serve, register, and bundle clyde's session-catalog MCP tools (local stdio MCP).
    ///
    /// `clyde mcp serve` speaks JSON-RPC on stdin/stdout for an MCP host (e.g. Claude Code); it is
    /// spawned, not run by hand. `register`/`unregister`/`status` self-manage the Claude config
    /// entry (`{"command":"<abs clyde>","args":["mcp","serve"]}`); `bundle` packages a `.mcpb`. The
    /// subcommand surface and stdio/logging discipline come from the shared `mcp-io` library.
    Mcp(mcp_io::McpCmd),
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// Full-text search over sessions, ranked.
    Search(SearchArgs),
    /// List sessions by repo / date / tag / model.
    Ls(LsArgs),
    /// Resume a session in the directory it originally ran in.
    ///
    /// Resolves the session's recorded working directory, changes into it, and replaces the clyde
    /// process with `claude --resume <id>` (fork/exec). On exit you are returned to your original
    /// shell prompt and directory.
    ///
    /// To forward extra flags to `claude`, separate them with a literal `--`:
    ///
    ///   clyde session resume <id> -- --model opus
    ///
    /// Omitting the `--` will cause a parse error; clyde does not interpret claude's flags.
    Resume(ResumeArgs),
    /// Set tags on a session (replaces existing).
    Tag(TagArgs),
    /// Reindex the catalog (incremental).
    Reindex(ReindexArgs),
    /// Stage durable copies of dormant transcripts.
    Stage(StageArgs),
    /// Enrich dormant sessions with tags + summary.
    Enrich(EnrichArgs),
    /// Report enrichment health.
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
    /// Result ordering: relevance (BM25, default) or recency (most-recent first).
    #[arg(long, value_enum, default_value_t = SortOrder::Relevance, ignore_case = true)]
    pub sort: SortOrder,
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
pub struct ResumeArgs {
    /// Session id or a unique prefix of it.
    pub id: String,
    /// Skip the lazy reindex before resolving.
    #[arg(long)]
    pub no_reindex: bool,
    /// Extra flags forwarded verbatim to `claude` after `--resume <id>`.
    ///
    /// Requires a literal `--` separator before the flags:
    ///
    ///   clyde session resume <id> -- --model opus
    ///
    /// Without `--`, clyde will reject flags it does not recognize. This is intentional:
    /// it prevents claude flags from being silently misinterpreted by clyde's own parser.
    #[arg(last = true)]
    pub extra: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct TagArgs {
    /// Session id or a unique prefix of it.
    pub id: String,
    /// Tags to set (space-separated; replaces any existing tags). Omit to clear all tags and
    /// reset provenance so a later `enrich` pass can re-tag the session.
    #[arg(num_args = 0..)]
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
    /// Treat a session as dormant once idle this long (e.g. 7d, 24h).
    #[arg(long, default_value = "7d")]
    pub dormant_after: String,
    /// Stage every non-archived session regardless of dormancy.
    #[arg(long)]
    pub all: bool,
}

#[derive(clap::Args, Debug)]
pub struct EnrichArgs {
    /// Enrich one session by id or prefix (manual; bypasses gating).
    pub id: Option<String>,
    /// Re-enrich every eligible session (overwrites manual tags).
    #[arg(long)]
    pub all: bool,
    /// Treat a session as dormant once idle this long (e.g. 7d, 24h).
    #[arg(long, default_value = "7d")]
    pub dormant_after: String,
    /// Preview the gate's decisions without sending anything off-machine.
    #[arg(long)]
    pub dry_run: bool,
    /// Dry-run only: write each redacted payload here for inspection.
    #[arg(long)]
    pub show_payload: Option<PathBuf>,
    /// Per-session attempt cap before a repeatedly-failing session stops being retried.
    #[arg(long, default_value_t = sessions::enrich::DEFAULT_MAX_ATTEMPTS)]
    pub max_attempts: i64,
    /// Halt the sweep once cumulative tokens (in + out) reach this budget.
    #[arg(long)]
    pub budget_tokens: Option<u64>,
}

/// The subcommand path whose help is being requested, or `None` when this is not a help
/// invocation. Recognizes both `clyde <path...> -h/--help` and clap's `clyde help <path...>`
/// form. Skips clyde's value-taking global flags so `clyde -l debug report --help` still maps to
/// `["report"]`; anchors on the positionals so `clyde session search report --help` maps to
/// `["session","search","report"]` (which matches no tool-bearing subcommand) rather than
/// `["report"]`.
///
/// Used to decide whether — and which — REQUIRED TOOLS `after_help` to attach. Those blocks spawn
/// a `--version` probe per tool, so they must be built only when that specific help is requested,
/// never on a normal run.
pub(crate) fn help_target(argv: &[String]) -> Option<Vec<String>> {
    // clyde's value-taking global flags: the following token is their value, not a subcommand.
    const VALUE_FLAGS: &[&str] = &["-l", "--log-level", "--db"];
    let mut positionals = Vec::new();
    let mut saw_help_flag = false;
    let mut i = 1; // skip argv[0] (program name)
    while let Some(arg) = argv.get(i) {
        if arg == "--" {
            break;
        }
        if arg == "-h" || arg == "--help" {
            // `--help` is an early-exit flag: clap renders help for the command parsed SO FAR and
            // ignores anything after it. So stop here — `clyde --help report` targets root help
            // (no positionals yet), not report.
            saw_help_flag = true;
            break;
        } else if VALUE_FLAGS.contains(&arg.as_str()) {
            i += 2; // skip the flag and its value
            continue;
        } else if !arg.starts_with('-') {
            positionals.push(arg.clone());
        }
        i += 1;
    }
    // `clyde help <path...>` — the help subcommand names its target explicitly.
    if positionals.first().map(String::as_str) == Some("help") {
        let path = positionals[1..].to_vec();
        return (!path.is_empty()).then_some(path);
    }
    // `clyde <path...> --help` — the positionals ARE the target path.
    (saw_help_flag && !positionals.is_empty()).then_some(positionals)
}

#[cfg(test)]
mod tests;
