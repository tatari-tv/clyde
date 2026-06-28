use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::LazyLock;

static HELP_TEXT: LazyLock<String> = LazyLock::new(get_tool_validation_help);

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
    /// `--since`/`--until` window, optionally titles untitled sessions via Haiku, and
    /// writes a timestamped JSON file under the XDG data dir.
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

    /// Write the JSON report to this path instead of the default timestamped file
    /// under `$XDG_DATA_HOME/claude-report/`.
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

    /// Path to a Jinja2/Tera template that overrides the built-in Markdown template.
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

    /// PDF converter binary to use when `--pdf` is set (default: `wkhtmltopdf`).
    /// Must be on `PATH`.
    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,
}

#[derive(clap::Args, Debug)]
pub struct MergeArgs {
    /// Two or more collected JSON report files to merge. Each must share the same
    /// schema version. Providing a single file is accepted (identity operation).
    pub inputs: Vec<PathBuf>,

    /// Write the merged JSON report to this path. When omitted, the merged report is
    /// streamed to stdout so `report merge a.json b.json | jq` works.
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
        ("pandoc", "--version", "report render --pdf"),
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
    help.push_str("\nLogs: ~/.local/share/claude-report/logs/claude-report.log");
    help
}

#[cfg(test)]
mod tests;
