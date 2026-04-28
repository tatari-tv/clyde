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

use eyre::{Result, bail};
use std::path::PathBuf;

pub use config::{Config, ResolvedCommand};

#[derive(Debug)]
pub struct RunResult {
    pub sessions_emitted: usize,
    pub output_path: PathBuf,
}

pub fn run(config: &Config) -> Result<RunResult> {
    match &config.command {
        ResolvedCommand::Scan(scan_cfg) => {
            let files = scan::find_session_files(&scan_cfg.projects_dir)?;
            log::info!("run: discovered {} session files", files.len());
            Ok(RunResult {
                sessions_emitted: 0,
                output_path: scan_cfg.output.clone(),
            })
        }
        ResolvedCommand::Render(render_cfg) => render::run(render_cfg),
        ResolvedCommand::Merge(_) => {
            bail!("`cr merge` is not implemented in this release");
        }
    }
}
