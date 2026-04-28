#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod cli;
pub mod config;
pub mod parse;
pub mod render;
pub mod repo;
pub mod report;
pub mod scan;
pub mod session;
pub mod title;

use crate::config::ScanConfig;
use crate::parse::ParseResult;
use eyre::{Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub use config::{Config, ResolvedCommand};

#[derive(Debug)]
pub struct RunResult {
    pub sessions_emitted: usize,
    pub output_path: PathBuf,
}

pub fn run(config: &Config) -> Result<RunResult> {
    match &config.command {
        ResolvedCommand::Scan(cfg) => run_scan(cfg),
        ResolvedCommand::Render(cfg) => render::run(cfg),
        ResolvedCommand::Merge(_) => {
            bail!("`cr merge` is not implemented in this release");
        }
    }
}

fn run_scan(cfg: &ScanConfig) -> Result<RunResult> {
    let files = scan::find_session_files(&cfg.projects_dir)?;
    log::info!("run_scan: discovered {} session files", files.len());

    let parsed: HashMap<PathBuf, ParseResult> = files
        .par_iter()
        .filter_map(|f| match parse::parse_jsonl_file(&f.path) {
            Ok(r) => Some((f.path.clone(), r)),
            Err(e) => {
                log::warn!("parse failed for {}: {}", f.path.display(), e);
                None
            }
        })
        .collect();

    let existing_titles = report::load_existing_titles(&cfg.output);
    let mut resolver = repo::Resolver::new();

    let summaries = session::fold(
        &files,
        &parsed,
        cfg.since,
        cfg.until,
        cfg.no_rollup,
        &mut resolver,
        &existing_titles,
    );

    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let count = report::write_yaml(&cfg.output, &summaries, cfg.since, cfg.until, &host)?;

    Ok(RunResult {
        sessions_emitted: count,
        output_path: cfg.output.clone(),
    })
}

#[cfg(test)]
mod tests;
