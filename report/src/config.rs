use crate::cli::CollectArgs;
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use common::DateTz;
use eyre::{Result, bail};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub command: ResolvedCommand,
    pub log_level: String,
}

#[derive(Debug, Clone)]
pub enum ResolvedCommand {
    Collect(CollectConfig),
    Render(RenderConfig),
    Merge(MergeConfig),
}

#[derive(Debug, Clone)]
pub struct CollectConfig {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub output: PathBuf,
    pub projects_dir: PathBuf,
    pub no_rollup: bool,
    pub skip_title: bool,
}

#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub input: PathBuf,
    /// Explicit output path. When `None`, render::run resolves a default of the form
    /// `./<YYYY-MM>-claude-report.{md,pdf}` using the `since` field from the input JSON.
    pub output: Option<PathBuf>,
    pub pdf: bool,
    pub template: Option<PathBuf>,
    pub prompt: Option<PathBuf>,
    pub include_tradeoffs: bool,
    pub pdf_engine: String,
}

#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub inputs: Vec<PathBuf>,
}

/// Default *input* path for `cr render` when `-i` is omitted. Collect no longer writes here
/// by default (see `default_collect_output`); render's default input is intentionally left
/// as the legacy CWD path and is out of Phase 0 scope.
const DEFAULT_RENDER_INPUT: &str = "./claude-report.json";

/// Resolve a parsed `cr`/`clyde report` subcommand into its validated [`ResolvedCommand`].
/// Split out of the former `TryFrom<Cli>` so `report::run` can own building the [`Config`] from
/// the nested [`crate::cli::ReportArgs`] plus the common globals.
pub fn resolve_command(command: crate::cli::Command, tz: DateTz) -> Result<ResolvedCommand> {
    let resolved = match command {
        crate::cli::Command::Collect(args) => ResolvedCommand::Collect(collect_config_from_args(args, tz)?),
        crate::cli::Command::Render(args) => {
            let pdf = args.pdf;
            let input = args.input.unwrap_or_else(|| PathBuf::from(DEFAULT_RENDER_INPUT));
            ResolvedCommand::Render(RenderConfig {
                input,
                output: args.output,
                pdf,
                template: args.template,
                prompt: args.prompt,
                include_tradeoffs: args.include_tradeoffs,
                pdf_engine: args.pdf_engine,
            })
        }
        crate::cli::Command::Merge(args) => ResolvedCommand::Merge(MergeConfig { inputs: args.inputs }),
    };
    Ok(resolved)
}

fn collect_config_from_args(args: CollectArgs, tz: DateTz) -> Result<CollectConfig> {
    // Shared parser (common::parse_since) so `--since 2d` (a relative span) now works for report,
    // not just RFC 3339 / YYYY-MM-DD. The bare-date midnight convention follows the configured tz.
    let since = match args.since {
        Some(s) => common::parse_since(&s, tz)?,
        None => first_of_month_local_midnight(),
    };
    let until = match args.until {
        Some(s) => common::parse_since(&s, tz)?,
        None => Utc::now(),
    };
    if since > until {
        bail!("--since ({}) is after --until ({})", since, until);
    }
    let output = match args.output {
        Some(p) => p,
        None => default_collect_output()?,
    };
    let projects_dir = args
        .projects_dir
        .or_else(default_projects_dir)
        .ok_or_else(|| eyre::eyre!("could not determine ~/.claude/projects/ path"))?;
    if !projects_dir.is_dir() {
        bail!(
            "projects directory does not exist: {}\nPass --projects-dir <path> to override.",
            projects_dir.display()
        );
    }
    Ok(CollectConfig {
        since,
        until,
        output,
        projects_dir,
        no_rollup: args.no_rollup,
        skip_title: args.skip_title,
    })
}

fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Default `cr collect` output: a timestamped file under the XDG data dir, so repeated runs
/// never clobber a prior report. Mirrors rkvr's `chrono::Local` `%Y-%m-%d-%H%M%S` stamp.
/// Shape: `<xdg-data>/claude-report/claude-report-YYYY-MM-DD-HHMMSS.json`.
fn default_collect_output() -> Result<PathBuf> {
    let dir = xdg_data_dir()
        .ok_or_else(|| eyre::eyre!("could not determine XDG data dir (set HOME or XDG_DATA_HOME)"))?
        .join("claude-report");
    let stamp = Local::now().format("%Y-%m-%d-%H%M%S");
    Ok(dir.join(format!("claude-report-{stamp}.json")))
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// We deliberately do NOT use the `dirs` config/data helpers: those honor
/// `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` only on Linux. On macOS they resolve via system
/// APIs and return `~/Library/...`, ignoring the env vars. These helpers resolve to the
/// same XDG layout on every platform.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

fn first_of_month_local_midnight() -> DateTime<Utc> {
    let now = Local::now();
    let date = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).expect("first of current month is always valid");
    let dt = NaiveDateTime::new(date, NaiveTime::MIN);
    let local = Local
        .from_local_datetime(&dt)
        .single()
        .or_else(|| Local.from_local_datetime(&dt).earliest())
        .expect("local midnight on the 1st resolves to a real instant");
    local.with_timezone(&Utc)
}

#[cfg(test)]
mod tests;
