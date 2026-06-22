//! Locate session transcripts under `~/.claude/projects`.
//!
//! The layout (and the rollup contract) mirrors `cr`: each project dir holds top-level
//! `<uuid>.jsonl` parent sessions, and a sibling `<uuid>/subagents/*.jsonl` tree of subagent
//! transcripts that roll up into the parent `<uuid>`. Unlike `cr`, a non-UUID stem here is
//! warned-and-skipped rather than fatal — the design contract is "skip-and-log, never crash
//! the reindex" so one malformed directory can't abort a whole scan.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eyre::Result;
use log::{debug, info, trace, warn};
use regex::Regex;

use crate::model::{SessionFile, SessionFileKind};

const UUID_V4_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

fn uuid_v4_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(UUID_V4_PATTERN).expect("UUID-v4 pattern is a valid regex"))
}

/// Walk `projects_dir` and return every parent and subagent transcript found.
///
/// Returns an empty vec (not an error) when `projects_dir` does not exist, so a machine with no
/// Claude history is not a failure. Unreadable entries and non-UUID names are warned and skipped.
pub fn find_session_files(projects_dir: &Path) -> Result<Vec<SessionFile>> {
    debug!("scan::find_session_files: projects_dir={}", projects_dir.display());

    if !projects_dir.exists() {
        warn!("scan::find_session_files: {} does not exist", projects_dir.display());
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for project in read_dir_or_warn(projects_dir, "projects directory") {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        scan_project(&project_path, &mut files);
    }

    info!("scan::find_session_files: discovered {} files", files.len());
    Ok(files)
}

fn scan_project(project_path: &Path, files: &mut Vec<SessionFile>) {
    trace!("scan::scan_project: project_path={}", project_path.display());
    for entry in read_dir_or_warn(project_path, "project directory") {
        let entry_path = entry.path();

        if entry_path.is_file() && entry_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let stem = entry_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if !uuid_v4_regex().is_match(stem) {
                warn!(
                    "scan::scan_project: skipping non-UUID parent JSONL {}",
                    entry_path.display()
                );
                continue;
            }
            push_if_nonempty(files, entry_path.clone(), stem, SessionFileKind::Parent);
            continue;
        }

        if entry_path.is_dir() {
            scan_subagents(&entry_path, files);
        }
    }
}

fn scan_subagents(session_dir: &Path, files: &mut Vec<SessionFile>) {
    let stem = session_dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    let subagents_dir = session_dir.join("subagents");
    if !subagents_dir.is_dir() {
        return;
    }
    if !uuid_v4_regex().is_match(stem) {
        warn!(
            "scan::scan_subagents: skipping non-UUID session dir {}",
            session_dir.display()
        );
        return;
    }
    for sub in read_dir_or_warn(&subagents_dir, "subagents directory") {
        let sub_path = sub.path();
        if !sub_path.is_file() || sub_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        push_if_nonempty(files, sub_path, stem, SessionFileKind::Subagent);
    }
}

fn push_if_nonempty(files: &mut Vec<SessionFile>, path: PathBuf, group_id: &str, kind: SessionFileKind) {
    match fs::metadata(&path) {
        Ok(m) if m.len() == 0 => {
            trace!("scan: skipping empty file {}", path.display());
        }
        Ok(_) => files.push(SessionFile {
            path,
            group_id: group_id.to_string(),
            kind,
        }),
        Err(e) => warn!("scan: error reading metadata for {}: {}", path.display(), e),
    }
}

fn read_dir_or_warn(path: &Path, label: &str) -> Vec<fs::DirEntry> {
    let mut out = Vec::new();
    let iter = match fs::read_dir(path) {
        Ok(it) => it,
        Err(e) => {
            warn!("scan: failed to read {} {}: {}", label, path.display(), e);
            return out;
        }
    };
    for entry in iter {
        match entry {
            Ok(e) => out.push(e),
            Err(e) => warn!("scan: error reading entry under {}: {}", path.display(), e),
        }
    }
    out
}

#[cfg(test)]
mod tests;
