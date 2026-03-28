use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ccu",
    about = "Claude Code cost and usage tracker",
    version = env!("GIT_DESCRIBE"),
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Override ~/.claude/projects/ scan path
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Filter to a specific model
    #[arg(long)]
    pub model: Option<String>,

    /// Set log level (trace, debug, info, warn, error)
    #[arg(long, env = "CCU_LOG_LEVEL")]
    pub log_level: Option<String>,

    /// Skip the cost cache, recompute from JSONL
    #[arg(long)]
    pub no_cache: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show cost for a specific session (by ID or "current")
    Session {
        /// Session ID or "current"
        id: String,
    },
    /// Show today's total cost (default)
    Today {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,

        /// Output only the total cost as a plain number
        #[arg(short, long)]
        total: bool,

        /// Show per-session breakdown
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show yesterday's total cost
    Yesterday {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,

        /// Output only the total cost as a plain number
        #[arg(short, long)]
        total: bool,

        /// Show per-session breakdown
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show daily costs for a date range
    Daily {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,

        /// Output only the total cost as a plain number
        #[arg(short, long)]
        total: bool,

        /// Number of days to show
        #[arg(short, long, default_value = "7")]
        days: u32,

        /// Show partial-period-weighted average
        #[arg(short, long)]
        average: bool,

        /// Show inline bar charts and braille line chart
        #[arg(short, long)]
        graph: bool,
    },
    /// Show weekly cost summary (Sun-Sat weeks, clipped to Sunday)
    Weekly {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,

        /// Output only the total cost as a plain number
        #[arg(short, long)]
        total: bool,

        /// Number of weeks to show
        #[arg(short, long, default_value = "4")]
        weeks: u32,

        /// Show partial-period-weighted average
        #[arg(short, long)]
        average: bool,

        /// Show inline bar charts and braille line chart
        #[arg(short, long)]
        graph: bool,

        /// Rolling window (last N*7 days from today instead of clipping to Sunday)
        #[arg(short, long)]
        rolling: bool,
    },
    /// Show monthly cost summary (clipped to 1st of month)
    Monthly {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,

        /// Output only the total cost as a plain number
        #[arg(short, long)]
        total: bool,

        /// Number of months to show
        #[arg(short, long, default_value = "12", value_parser = clap::value_parser!(u32).range(1..))]
        months: u32,

        /// Show partial-period-weighted average
        #[arg(short, long)]
        average: bool,

        /// Show inline bar charts and braille line chart
        #[arg(short, long)]
        graph: bool,

        /// Rolling window (last N months from today instead of clipping to 1st)
        #[arg(short, long)]
        rolling: bool,
    },
    /// Install a Claude Code statusline
    Statusline {
        /// Name of the statusline to install (omit for default)
        name: Option<String>,

        /// List available statuslines
        #[arg(short, long)]
        list: bool,
    },
    /// Manage model pricing configuration
    Pricing {
        /// Fetch the live pricing page and check if embedded pricing may be stale
        #[arg(long)]
        check: bool,

        /// Display current pricing table
        #[arg(long)]
        show: bool,
    },
}
