use crate::RunResult;
use crate::config::ScanConfig;
use eyre::Result;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    Parent,
    Subagent,
}

#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub group_id: String,
    pub kind: SessionFileKind,
}

pub fn run(cfg: &ScanConfig) -> Result<RunResult> {
    log::info!("scan::run: projects_dir={}", cfg.projects_dir.display());
    Ok(RunResult {
        sessions_emitted: 0,
        output_path: cfg.output.clone(),
    })
}
