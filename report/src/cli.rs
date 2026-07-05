use crate::aggregate::DEFAULT_OUTLIERS;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::LazyLock;

static HELP_TEXT: LazyLock<String> = LazyLock::new(get_tool_validation_help);

/// Output format for `report render`, selected via `--format` (case-insensitive, kebab-case).
/// `markdown` (default) and `pdf` write locally (see `-o`); the two `marquee-*` variants publish
/// to marquee and print the resulting URL instead of writing a file.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum Format {
    #[default]
    Markdown,
    Pdf,
    MarqueeHtml,
    MarqueeMarkdown,
}

impl Format {
    /// The two publishing variants, whose output is a marquee URL rather than a local path.
    pub fn is_marquee(self) -> bool {
        matches!(self, Format::MarqueeHtml | Format::MarqueeMarkdown)
    }
}

/// Map the `clyde.yml` `render.format` config value onto the CLI [`Format`]. Lives here (not in
/// `common`) because the mapping's target type is owned by this crate.
impl From<common::config::FormatConfig> for Format {
    fn from(value: common::config::FormatConfig) -> Self {
        match value {
            common::config::FormatConfig::Markdown => Format::Markdown,
            common::config::FormatConfig::Pdf => Format::Pdf,
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

/// Standalone wrapper for the `cr` compat shim. Owns the common `--log-level` global (so
/// `cr --log-level ...` still works) and flattens [`ReportArgs`]. The `globals()` accessor is the
/// integration seam: it reconstructs [`common::Globals`] from this wrapper's own fields so the
/// shim and `clyde report` drive `report::run` through the exact same code path.
#[derive(Parser, Debug)]
#[command(
    name = "cr",
    about = "Scan Claude Code session JSONL files and emit a per-host JSON report",
    version = env!("GIT_DESCRIBE"),
    after_help = HELP_TEXT.as_str(),
    arg_required_else_help = true,
)]
pub struct ReportCli {
    #[arg(short = 'l', long, global = true, default_value = "info")]
    pub log_level: String,

    #[command(flatten)]
    pub args: ReportArgs,
}

impl ReportCli {
    /// Reconstruct the common globals from the shim wrapper's fields.
    pub fn globals(&self) -> common::Globals {
        common::Globals {
            log_level: Some(self.log_level.clone()),
        }
    }
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
    /// Render a collected JSON report into Markdown, PDF, or a published marquee post (`--format`).
    ///
    /// Reads the JSON produced by `collect` (default: `./claude-report.json`) and writes a
    /// human-readable Markdown summary. `--format pdf` converts it with the configured
    /// `--pdf-engine`; `--format marquee-markdown` / `marquee-html` publish it to marquee and
    /// print the resulting URL.
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

    /// Write rendered Markdown (or PDF) to this path. When omitted, the output is
    /// placed beside the input file with a `.md` (or `.pdf`) extension. Not valid with the
    /// `marquee-*` formats, whose output is a published URL.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Output format: `markdown`, `pdf`, `marquee-markdown`, or `marquee-html`. When omitted,
    /// falls back to the `render.format` value in `clyde.yml`, and to `markdown` if that too is
    /// unset. `markdown`/`pdf` write locally (see `-o`); the `marquee-*` variants publish to
    /// marquee and print the URL. `pdf` and `marquee-html` require `pandoc`; the `marquee-*`
    /// variants require the `marquee` CLI with an authenticated session.
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

    /// Path to a file containing the LLM prompt used when generating session summaries.
    /// Overrides the built-in prompt.
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

struct ToolStatus {
    version: String,
    icon: &'static str,
}

fn check_tool(tool: &str, version_arg: &str) -> ToolStatus {
    match ProcessCommand::new(tool).arg(version_arg).output() {
        Ok(output) if output.status.success() => {
            let body = String::from_utf8_lossy(&output.stdout);
            let version = extract_version(&body);
            ToolStatus {
                version: if version.is_empty() {
                    "installed".to_string()
                } else {
                    version
                },
                icon: "✅",
            }
        }
        _ => ToolStatus {
            version: "not found".to_string(),
            icon: "❌",
        },
    }
}

fn extract_version(output: &str) -> String {
    let Some(line) = output.lines().next() else {
        return String::new();
    };
    for word in line.split_whitespace() {
        if let Some(v) = looks_like_version(word.trim_start_matches('v')) {
            return v.to_string();
        }
        // Handle single-token formats like `jq-1.8.1` where the version
        // sits after a dash with no whitespace before it.
        if let Some((_, suffix)) = word.rsplit_once('-')
            && let Some(v) = looks_like_version(suffix)
        {
            return v.to_string();
        }
    }
    if let Some(v) = looks_like_version(line.trim()) {
        return v.to_string();
    }
    String::new()
}

fn looks_like_version(s: &str) -> Option<&str> {
    if s.contains('.') && s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some(s)
    } else {
        None
    }
}

fn get_tool_validation_help() -> String {
    let tools: &[(&str, &str, &str)] = &[
        ("persona", "--version", "report render: persona block in context"),
        ("pandoc", "--version", "report render --format pdf / marquee-html"),
        (
            "marquee",
            "--version",
            "report render --format marquee-html / marquee-markdown",
        ),
        ("git", "--version", "report collect: repo detection"),
        ("jq", "--version", "report collect: query JSON report output"),
    ];

    let entries: Vec<(ToolStatus, &str, &str)> = tools
        .iter()
        .map(|(name, arg, purpose)| (check_tool(name, arg), *name, *purpose))
        .collect();

    let max_name = entries.iter().map(|(_, n, _)| n.len()).max().unwrap_or(0);
    let max_ver = entries.iter().map(|(s, _, _)| s.version.len()).max().unwrap_or(0);

    let mut help = String::from("REQUIRED TOOLS:\n");
    for (status, name, purpose) in &entries {
        help.push_str(&format!(
            "  {} {:<name_w$}  {:>ver_w$}  ({})\n",
            status.icon,
            name,
            status.version,
            purpose,
            name_w = max_name,
            ver_w = max_ver,
        ));
    }
    help.push_str(&format!("\nLogs: {}", crate::log_file_path().display()));
    help
}

#[cfg(test)]
mod tests;
