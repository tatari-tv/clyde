use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cr",
    about = "Scan Claude Code session JSONL files and emit a per-host YAML report",
    version = env!("GIT_DESCRIBE"),
    after_help = "Default subcommand is `collect`. Logs: ~/.local/share/claude-report/logs/claude-report.log",
)]
pub struct Cli {
    #[arg(short = 'l', long, global = true, default_value = "info")]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Collect(CollectArgs),
    Render(RenderArgs),
    Merge(MergeArgs),
}

#[derive(clap::Args, Debug, Default)]
pub struct CollectArgs {
    #[arg(long)]
    pub since: Option<String>,

    #[arg(long)]
    pub until: Option<String>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,

    #[arg(long)]
    pub projects_dir: Option<PathBuf>,

    #[arg(long)]
    pub no_rollup: bool,

    #[arg(long)]
    pub skip_title: bool,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    #[arg(short, long)]
    pub input: Option<PathBuf>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,

    #[arg(long)]
    pub pdf: bool,

    #[arg(long)]
    pub template: Option<PathBuf>,

    #[arg(long)]
    pub prompt: Option<PathBuf>,

    #[arg(long)]
    pub include_tradeoffs: bool,

    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,
}

#[derive(clap::Args, Debug)]
pub struct MergeArgs {
    pub inputs: Vec<PathBuf>,
}
