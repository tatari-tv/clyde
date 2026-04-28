use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cr",
    about = "Scan Claude Code session JSONL files and emit a per-host YAML report",
    version = env!("GIT_DESCRIBE"),
    after_help = "Logs are written to: ~/.local/share/claude-report/logs/claude-report.log",
    args_conflicts_with_subcommands = true,
)]
pub struct Cli {
    #[arg(short = 'l', long, global = true, default_value = "info")]
    pub log_level: String,

    #[command(flatten)]
    pub scan: ScanArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Args, Debug, Default)]
pub struct ScanArgs {
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

#[derive(Subcommand, Debug)]
pub enum Command {
    Render(RenderArgs),
    Merge(MergeArgs),
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

    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,
}

#[derive(clap::Args, Debug)]
pub struct MergeArgs {
    pub inputs: Vec<PathBuf>,
}
