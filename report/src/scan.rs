use eyre::{Result, bail};
use log::{debug, info, warn};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

const UUID_V4_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

fn uuid_v4_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(UUID_V4_PATTERN).expect("UUID-v4 pattern is a valid regex"))
}

pub fn find_session_files(projects_dir: &Path) -> Result<Vec<SessionFile>> {
    debug!("scan::find_session_files: projects_dir={}", projects_dir.display());

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for project in read_dir_or_warn(projects_dir, "projects directory")? {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }

        for entry in read_dir_or_warn(&project_path, "project directory")? {
            let entry_path = entry.path();

            if entry_path.is_file() && entry_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let stem = entry_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if !uuid_v4_regex().is_match(stem) {
                    bail!(
                        "scan: parent JSONL stem is not a UUID-v4: {} (refusing to misclassify as a parent session)",
                        entry_path.display()
                    );
                }
                if let Some(file) = make_parent(entry_path.clone(), stem) {
                    files.push(file);
                }
                continue;
            }

            if entry_path.is_dir() {
                let stem = entry_path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
                let subagents_dir = entry_path.join("subagents");
                if !subagents_dir.is_dir() {
                    continue;
                }
                if !uuid_v4_regex().is_match(stem) {
                    bail!(
                        "scan: parent session directory is not a UUID-v4: {} (refusing to misclassify subagents)",
                        entry_path.display()
                    );
                }
                for sub in read_dir_or_warn(&subagents_dir, "subagents directory")? {
                    let sub_path = sub.path();
                    if !sub_path.is_file() || sub_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(file) = make_subagent(sub_path, stem) {
                        files.push(file);
                    }
                }
            }
        }
    }

    info!("scan::find_session_files: discovered {} files", files.len());
    Ok(files)
}

fn make_parent(path: PathBuf, stem: &str) -> Option<SessionFile> {
    if is_empty_file(&path)? {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: stem.to_string(),
        kind: SessionFileKind::Parent,
    })
}

fn make_subagent(path: PathBuf, parent_stem: &str) -> Option<SessionFile> {
    if is_empty_file(&path)? {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: parent_stem.to_string(),
        kind: SessionFileKind::Subagent,
    })
}

fn is_empty_file(path: &Path) -> Option<bool> {
    match fs::metadata(path) {
        Ok(m) => Some(m.len() == 0),
        Err(e) => {
            warn!("scan: error reading metadata for {}: {}", path.display(), e);
            None
        }
    }
}

fn read_dir_or_warn(path: &Path, label: &str) -> Result<Vec<fs::DirEntry>> {
    let mut out = Vec::new();
    let iter = fs::read_dir(path).map_err(|e| eyre::eyre!("failed to read {} {}: {}", label, path.display(), e))?;
    for entry in iter {
        match entry {
            Ok(e) => out.push(e),
            Err(e) => warn!("scan: error reading entry under {}: {}", path.display(), e),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
