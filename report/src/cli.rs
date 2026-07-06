use crate::aggregate::DEFAULT_OUTLIERS;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Output format for `report render`, selected via `--format` (case-insensitive, kebab-case).
/// `markdown`, `pdf`, and `html` write locally (see `-o`); the two `marquee-*` variants publish
/// to marquee and print the resulting URL instead of writing a file.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum Format {
    #[default]
    Markdown,
    Pdf,
    Html,
    MarqueeHtml,
    MarqueeMarkdown,
}

impl Format {
    /// The two publishing variants, whose output is a marquee URL rather than a local path.
    pub fn is_marquee(self) -> bool {
        matches!(self, Format::MarqueeHtml | Format::MarqueeMarkdown)
    }

    /// The two model-authored-HTML variants, which share the html-source render pipeline (context
    /// block -> `report-html.pmt` -> opus -> a complete HTML document; no pandoc) rather than the
    /// markdown-source pipeline every other format uses.
    pub fn is_html_source(self) -> bool {
        matches!(self, Format::Html | Format::MarqueeHtml)
    }
}

/// Map the `clyde.yml` `render.format` config value onto the CLI [`Format`]. Lives here (not in
/// `common`) because the mapping's target type is owned by this crate.
impl From<common::config::FormatConfig> for Format {
    fn from(value: common::config::FormatConfig) -> Self {
        match value {
            common::config::FormatConfig::Markdown => Format::Markdown,
            common::config::FormatConfig::Pdf => Format::Pdf,
            common::config::FormatConfig::Html => Format::Html,
            common::config::FormatConfig::MarqueeHtml => Format::MarqueeHtml,
            common::config::FormatConfig::MarqueeMarkdown => Format::MarqueeMarkdown,
        }
    }
}

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
    /// Render a collected JSON report into Markdown, PDF, HTML, or a published marquee post
    /// (`--format`).
    ///
    /// Reads the JSON produced by `collect` (default: `./claude-report.json`) and writes a
    /// human-readable Markdown summary. `--format pdf` converts it with the configured
    /// `--pdf-engine`; `--format html` writes a self-contained HTML file locally; `--format
    /// marquee-markdown` / `marquee-html` publish it to marquee and print the resulting URL.
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

    /// Skip the outcome extraction pass (commits, PRs opened, Confluence/Jira writes, Slack
    /// messages, files edited) mined from session transcripts. Skips the extra per-file read;
    /// the produced report carries `outcomes-enabled: false` and no `outcomes` fields anywhere.
    /// Default: extraction on.
    #[arg(long)]
    pub no_outcomes: bool,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    /// Path to the collected JSON report to render (default: `./claude-report.json`).
    #[arg(short, long)]
    pub input: Option<PathBuf>,

    /// Write rendered Markdown, PDF, or HTML to this path. When omitted, the output defaults to
    /// `./<YYYY-MM>-claude-report.{md,pdf,html}` in the current directory (month derived from the
    /// report's `since`). Not valid with the `marquee-*` formats, whose output is a published URL.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Output format: `markdown`, `pdf`, `html`, `marquee-markdown`, or `marquee-html`. When
    /// omitted, falls back to the `render.format` value in `clyde.yml`, and to `markdown` if that
    /// too is unset. `markdown`/`pdf`/`html` write locally (see `-o`); the `marquee-*` variants
    /// publish to marquee and print the URL. `pdf` requires `pandoc`; the `marquee-*` variants
    /// require the `marquee` CLI with an authenticated session. `html`/`marquee-html` are
    /// model-authored (no pandoc involved) and require `ANTHROPIC_API_KEY`; there is no offline
    /// path for them. Not valid with `--template` for `html`/`marquee-html` (the offline template
    /// produces markdown).
    #[arg(long, value_enum, ignore_case = true)]
    pub format: Option<Format>,

    /// Target marquee space for the `marquee-*` formats (defaults to your personal ~space).
    /// Ignored by `markdown`/`pdf`.
    #[arg(long)]
    pub space: Option<String>,

    /// Path to a template that overrides the built-in Markdown template. Rendering is
    /// plain `{{token}}` string replacement over exactly six placeholders: `{{host}}`,
    /// `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`,
    /// `{{total-spend}}`. No other tokens, loops, or conditionals are supported.
    #[arg(long)]
    pub template: Option<PathBuf>,

    /// Path to a file overriding the built-in LLM prompt. Dispatched by the resolved format's
    /// source family: `markdown`/`pdf`/`marquee-markdown` get the markdown report prompt;
    /// `html`/`marquee-html` get the HTML dashboard prompt.
    #[arg(long)]
    pub prompt: Option<PathBuf>,

    /// Include the "Tradeoffs" section in each session summary (omitted by default to
    /// keep reports concise).
    #[arg(long)]
    pub include_tradeoffs: bool,

    /// PDF engine to use when `--format pdf` is set (default: `wkhtmltopdf`), passed to pandoc
    /// as `--pdf-engine`; `pandoc` is the required binary that must be on `PATH`.
    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,

    /// Number of top-spend sessions to include in the outlier table.
    #[arg(long, default_value_t = DEFAULT_OUTLIERS)]
    pub outliers: usize,
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

#[cfg(test)]
mod tests;
