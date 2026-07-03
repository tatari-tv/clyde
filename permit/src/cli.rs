use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::sync::LazyLock;

/// Renders from [`crate::log_file_path`] so `--help` can never drift from the path the logger
/// actually writes (Phase 8, D3: log paths declared outside the behavior-exact shim surface).
static HELP_TEXT: LazyLock<String> =
    LazyLock::new(|| format!("Logs are written to: {}", crate::log_file_path().display()));

/// The permit command surface, nested under `clyde permit ...`. Derives `Args` (not `Parser`) so
/// it can be a `Subcommand` payload in the clyde umbrella. permit has no common globals of its
/// own (it never exposed `--log-level`).
#[derive(Args)]
pub struct PermitArgs {
    #[command(subcommand)]
    pub command: Command,
}

/// Standalone wrapper for the `claude-permit` compat shim. permit never accepted `--log-level`,
/// so the wrapper adds no common globals; `globals()` returns the default (log level unset),
/// which preserves the pre-merge `RUST_LOG`/`env_logger` default behavior exactly. `clyde permit`
/// still drives the level via clyde's own top-level `--log-level`.
#[derive(Parser)]
#[command(
    name = "claude-permit",
    about = "Manage Claude Code permission hygiene",
    version = env!("GIT_DESCRIBE"),
    after_help = HELP_TEXT.as_str()
)]
pub struct PermitCli {
    #[command(flatten)]
    pub args: PermitArgs,
}

impl PermitCli {
    /// Reconstruct the common globals from the shim wrapper. permit carries none, so this is the
    /// default (`log_level: None`).
    pub fn globals(&self) -> common::Globals {
        common::Globals::default()
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Log a permission event from hook JSON (reads stdin)
    Log,

    /// Audit current permission rules and classify by risk
    Audit {
        /// Override settings.json path
        #[arg(long)]
        settings: Option<PathBuf>,

        /// Override settings.local.json path
        #[arg(long)]
        settings_local: Option<PathBuf>,

        /// Output format: table, json, markdown
        #[arg(long, default_value = "table")]
        format: String,

        /// Filter by risk tier: safe, moderate, dangerous (cannot be combined with --apply)
        #[arg(long, conflicts_with = "apply")]
        risk: Option<String>,

        /// Apply recommendations and write changes. Optionally specify actions:
        /// promote, remove, deny, dupe (default: all). Cannot be combined with --risk.
        #[arg(long, value_name = "ACTION", num_args = 0.., conflicts_with = "risk")]
        apply: Option<Vec<String>>,

        /// Rule patterns to filter output (exact, prefix, or substring match)
        #[arg(value_name = "PATTERN")]
        patterns: Vec<String>,
    },

    /// Suggest promotions based on usage patterns
    Suggest {
        /// Min observations to trigger suggestion
        #[arg(long, default_value = "3")]
        threshold: u32,

        /// Min distinct sessions
        #[arg(long, default_value = "2")]
        sessions: u32,

        /// Output format: table, json, markdown
        #[arg(long, default_value = "table")]
        format: String,

        /// Rule patterns to filter output (exact, prefix, or substring match)
        #[arg(value_name = "PATTERN")]
        patterns: Vec<String>,
    },

    /// Session summary of permission activity
    Report {
        /// Session ID (default: latest)
        #[arg(long)]
        session: Option<String>,

        /// Output format: table, json, markdown
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// Prune old events from the database
    Clean {
        /// Delete events older than N days
        #[arg(long, default_value = "90")]
        older_than: u32,

        /// Show what would be deleted without deleting
        #[arg(long)]
        dry_run: bool,
    },

    /// Verify hook installation and DB connectivity
    Check,

    /// Install the PreToolUse hook into Claude Code settings
    Install {
        /// Override settings.json path
        #[arg(long)]
        settings: Option<PathBuf>,

        /// Actually write changes (default is dry-run)
        #[arg(long)]
        yes: bool,
    },

    /// Apply audit recommendations to settings files
    Apply {
        /// Apply "promote" recommendations (move safe local rules to global)
        #[arg(long)]
        promote: bool,

        /// Apply "remove" recommendations (delete dangerous local rules)
        #[arg(long)]
        remove: bool,

        /// Apply "deny" recommendations (remove denied patterns from allow lists)
        #[arg(long)]
        deny: bool,

        /// Apply all actionable recommendations (promote + remove + deny)
        #[arg(long)]
        all: bool,

        /// Override settings.json path
        #[arg(long)]
        settings: Option<PathBuf>,

        /// Override settings.local.json path
        #[arg(long)]
        settings_local: Option<PathBuf>,

        /// Actually write changes (default is dry-run)
        #[arg(long)]
        yes: bool,

        /// Skip creating backup files before writing
        #[arg(long)]
        no_backup: bool,
    },
}
