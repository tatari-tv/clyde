#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use clap::Parser;
use claude_report::cli::Cli;
use claude_report::{Config, ResolvedCommand, run};
use eyre::{Context, Result};
use log::LevelFilter;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

fn setup_logging(level: &str) -> Result<()> {
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-report")
        .join("logs");

    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    let log_file = log_dir.join("claude-report.log");

    let target = Box::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );

    let filter = LevelFilter::from_str(level).unwrap_or(LevelFilter::Info);

    env_logger::Builder::new()
        .filter_level(filter)
        .target(env_logger::Target::Pipe(target))
        .init();

    Ok(())
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    setup_logging(&cli.log_level).context("Failed to setup logging")?;
    let config = Config::try_from(cli).context("Failed to build configuration")?;

    if let ResolvedCommand::Merge(_) = config.command {
        eprintln!("cr: merge is not implemented in this release");
        return Ok(ExitCode::from(2));
    }

    let result = run(&config).context("cr failed")?;
    println!(
        "wrote {} sessions to {}",
        result.sessions_emitted,
        result.output_path.display()
    );
    Ok(ExitCode::SUCCESS)
}
