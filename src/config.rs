use crate::cli::{Cli, ScanArgs};
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use eyre::{Result, bail};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub command: ResolvedCommand,
    pub log_level: String,
}

#[derive(Debug, Clone)]
pub enum ResolvedCommand {
    Scan(ScanConfig),
    Render(RenderConfig),
    Merge(MergeConfig),
}

#[derive(Debug, Clone)]
pub struct ScanConfig {
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
    pub output: PathBuf,
    pub pdf: bool,
    pub template: Option<PathBuf>,
    pub pdf_engine: String,
}

#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub inputs: Vec<PathBuf>,
}

const DEFAULT_OUTPUT: &str = "./claude-report.yml";
const DEFAULT_RENDER_MD: &str = "./claude-report.md";
const DEFAULT_RENDER_PDF: &str = "./claude-report.pdf";

impl TryFrom<Cli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: Cli) -> Result<Self> {
        let log_level = cli.log_level.clone();
        let command = match cli.command {
            None => ResolvedCommand::Scan(scan_config_from_args(cli.scan)?),
            Some(crate::cli::Command::Render(args)) => {
                let pdf = args.pdf;
                let input = args.input.unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
                let output = args
                    .output
                    .unwrap_or_else(|| PathBuf::from(if pdf { DEFAULT_RENDER_PDF } else { DEFAULT_RENDER_MD }));
                ResolvedCommand::Render(RenderConfig {
                    input,
                    output,
                    pdf,
                    template: args.template,
                    pdf_engine: args.pdf_engine,
                })
            }
            Some(crate::cli::Command::Merge(args)) => ResolvedCommand::Merge(MergeConfig { inputs: args.inputs }),
        };
        Ok(Config { command, log_level })
    }
}

fn scan_config_from_args(args: ScanArgs) -> Result<ScanConfig> {
    let since = match args.since {
        Some(s) => parse_datetime(&s)?,
        None => first_of_month_local_midnight(),
    };
    let until = match args.until {
        Some(s) => parse_datetime(&s)?,
        None => Utc::now(),
    };
    if since > until {
        bail!("--since ({}) is after --until ({})", since, until);
    }
    let output = args.output.unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
    let projects_dir = args
        .projects_dir
        .or_else(default_projects_dir)
        .ok_or_else(|| eyre::eyre!("could not determine ~/.claude/projects/ path"))?;
    Ok(ScanConfig {
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

fn parse_datetime(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = NaiveDateTime::new(date, NaiveTime::MIN);
        let local = Local
            .from_local_datetime(&dt)
            .single()
            .or_else(|| Local.from_local_datetime(&dt).earliest())
            .ok_or_else(|| eyre::eyre!("date {} does not resolve to a local instant", s))?;
        return Ok(local.with_timezone(&Utc));
    }
    bail!("could not parse datetime '{}': expected RFC 3339 or YYYY-MM-DD", s)
}

#[cfg(test)]
mod tests;
