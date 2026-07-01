use clap::{Args, Subcommand};
use std::path::PathBuf;

/// The report command surface, nested under `clyde report ...`. Derives `Args` (not `Parser`)
/// so it can be a `Subcommand` payload in the clyde umbrella; carries no common globals (clyde
/// owns `--log-level`).
#[derive(Args, Debug)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scan session JSONL files and emit a per-host JSON usage report.
    ///
    /// Reads `~/.claude/projects/` (or `--projects-dir`), filters sessions by the
    /// `--since`/`--until` window, and optionally titles untitled sessions via Haiku.
    /// With `-o <path>`, writes the JSON report to that file; without `-o`, streams
    /// the JSON to stdout so `report collect | jq` works.
    Collect(CollectArgs),
    /// Render a collected JSON report into Markdown (or PDF via `--pdf`).
    ///
    /// Reads the JSON produced by `collect` (default: `./claude-report.json`) and writes
    /// a human-readable Markdown summary, optionally converting it to PDF with the
    /// configured `--pdf-engine`.
    Render(RenderArgs),
    /// Merge two or more collected JSON reports into one.
    ///
    /// Unions sessions from all inputs, recomputes totals, widens the
    /// `since`/`until` window to the min/max across inputs, and tags the output
    /// with a multi-host marker.
    Merge(MergeArgs),
}

#[derive(clap::Args, Debug)]
pub struct CollectArgs {
    /// Start of the collection window: RFC 3339 timestamp or `YYYY-MM-DD` date.
    /// When omitted, defaults to midnight on the first day of the current month.
    #[arg(long)]
    pub since: Option<String>,

    /// End of the collection window: RFC 3339 timestamp or `YYYY-MM-DD` date.
    /// When omitted, defaults to now.
    #[arg(long)]
    pub until: Option<String>,

    /// Write the JSON report to this path. When omitted, streams JSON to stdout so
    /// `report collect | jq` works.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Override the Claude projects directory (default: `~/.claude/projects`).
    #[arg(long)]
    pub projects_dir: Option<PathBuf>,

    /// Suppress sub-agent session rollup: count each sub-agent JSONL as its own
    /// session rather than folding it into the parent session's totals.
    #[arg(long)]
    pub no_rollup: bool,

    /// Skip the Haiku API call that titles untitled sessions. Useful when
    /// `ANTHROPIC_API_KEY` is not set or to avoid Haiku billing.
    #[arg(long)]
    pub skip_title: bool,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    /// Path to the collected JSON report to render (default: `./claude-report.json`).
    #[arg(short, long)]
    pub input: Option<PathBuf>,

    /// Write rendered Markdown (or PDF) to this path. When omitted, the output is
    /// placed beside the input file with a `.md` (or `.pdf`) extension.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Convert the rendered Markdown to PDF using `--pdf-engine` after rendering.
    #[arg(long)]
    pub pdf: bool,

    /// Path to a template that overrides the built-in Markdown template. Rendering is
    /// plain `{{token}}` string replacement over exactly six placeholders: `{{host}}`,
    /// `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`,
    /// `{{total-spend}}`. No other tokens, loops, or conditionals are supported.
    #[arg(long)]
    pub template: Option<PathBuf>,

    /// Path to a file containing the LLM prompt used when generating session summaries.
    /// Overrides the built-in prompt.
    #[arg(long)]
    pub prompt: Option<PathBuf>,

    /// Include the "Tradeoffs" section in each session summary (omitted by default to
    /// keep reports concise).
    #[arg(long)]
    pub include_tradeoffs: bool,

    /// PDF engine to use when `--pdf` is set (default: `wkhtmltopdf`), passed to pandoc
    /// as `--pdf-engine`; `pandoc` is the required binary that must be on `PATH`.
    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,
}

#[derive(clap::Args, Debug)]
pub struct MergeArgs {
    /// Two or more collected JSON report files to merge. Each must share the same
    /// schema version. Providing a single file is accepted (identity operation).
    pub inputs: Vec<PathBuf>,

    /// Write the merged JSON report to this path. With `-o <path>`, writes that file;
    /// without `-o`, streams JSON to stdout so `report merge a.json b.json | jq` works.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}
