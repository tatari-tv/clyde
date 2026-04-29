use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::LazyLock;

static HELP_TEXT: LazyLock<String> = LazyLock::new(get_tool_validation_help);

#[derive(Parser, Debug)]
#[command(
    name = "cr",
    about = "Scan Claude Code session JSONL files and emit a per-host JSON report",
    version = env!("GIT_DESCRIBE"),
    after_help = HELP_TEXT.as_str(),
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
                version: if version.is_empty() { "installed".to_string() } else { version },
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
    if let Some(line) = output.lines().next() {
        for word in line.split_whitespace() {
            let trimmed = word.trim_start_matches('v');
            if trimmed.contains('.') && trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return trimmed.to_string();
            }
        }
        let trimmed = line.trim();
        if trimmed.contains('.') && trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return trimmed.to_string();
        }
    }
    String::new()
}

fn get_tool_validation_help() -> String {
    let tools: &[(&str, &str, &str)] = &[
        ("persona", "--version", "cr render: persona block in context"),
        ("pandoc", "--version", "cr render --pdf"),
        ("git", "--version", "cr collect: repo detection"),
        ("jq", "--version", "cr collect: query JSON report output"),
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
    help.push_str("\nDefault subcommand is `collect`.");
    help.push_str("\nLogs: ~/.local/share/claude-report/logs/claude-report.log");
    help
}
