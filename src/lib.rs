#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod cli;
pub mod config;
pub mod parse;
pub mod pricing;
pub mod render;
pub mod repo;
pub mod report;
pub mod scan;
pub mod session;
pub mod title;

use crate::config::CollectConfig;
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
        ResolvedCommand::Collect(cfg) => run_collect(cfg),
        ResolvedCommand::Render(cfg) => render::run(cfg),
        ResolvedCommand::Merge(_) => {
            bail!("`cr merge` is not implemented in this release");
        }
    }
}

fn run_collect(cfg: &CollectConfig) -> Result<RunResult> {
    let files = scan::find_session_files(&cfg.projects_dir)?;
    log::info!("run_collect: discovered {} session files", files.len());

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

    let mut summaries = session::fold(
        &files,
        &parsed,
        cfg.since,
        cfg.until,
        cfg.no_rollup,
        &mut resolver,
        &existing_titles,
    );

    if !cfg.skip_title {
        title_untitled_sessions(&mut summaries);
    }

    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let count = report::write_yaml(&cfg.output, &summaries, cfg.since, cfg.until, &host)?;

    Ok(RunResult {
        sessions_emitted: count,
        output_path: cfg.output.clone(),
    })
}

fn title_untitled_sessions(summaries: &mut [session::SessionSummary]) {
    let api_key = match title::api_key_from_env() {
        Some(k) => k,
        None => {
            log::info!("run_collect: ANTHROPIC_API_KEY not set; skipping titling");
            return;
        }
    };

    let to_title: Vec<usize> = summaries
        .iter()
        .enumerate()
        .filter_map(|(i, s)| if s.title.is_none() && parent_jsonl(s).is_some() { Some(i) } else { None })
        .collect();

    if to_title.is_empty() {
        return;
    }

    log::info!("run_collect: titling {} sessions via Haiku", to_title.len());
    let titles: Vec<(usize, Option<String>)> = to_title
        .par_iter()
        .map(|&i| {
            let s = &summaries[i];
            let parent = match parent_jsonl(s) {
                Some(p) => p,
                None => return (i, None),
            };
            let prefix = match title::extract_prefix(parent) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("title: extract_prefix failed for {}: {}", parent.display(), e);
                    return (i, None);
                }
            };
            match title::haiku(&prefix, &api_key) {
                Ok(t) => (i, t),
                Err(e) => {
                    log::warn!("title::haiku failed for session {}: {}", s.session_id, e);
                    (i, None)
                }
            }
        })
        .collect();

    for (i, t) in titles {
        summaries[i].title = t;
    }
}

fn parent_jsonl(s: &session::SessionSummary) -> Option<&std::path::Path> {
    s.jsonl_paths
        .iter()
        .find(|p| !p.components().any(|c| c.as_os_str() == "subagents"))
        .map(|p| p.as_path())
}

#[cfg(test)]
mod tests;
