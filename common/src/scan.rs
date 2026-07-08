//! The single shared Claude-Code session-file scanner (Phase 5, cost-accuracy-verification).
//!
//! `cost` and `report` used to carry sibling copies of this discovery logic that drifted:
//! `report`'s was typed (parent/subagent grouping) and fail-loud (a UUID-v4 guard that `bail!`s on
//! a malformed dir); `cost`'s was the weaker, unguarded copy that additionally carried file mtime +
//! size for its date prefilter and cache hash. This module unifies them into ONE scanner both
//! crates consume, so the divergence class cannot recur.
//!
//! The unified [`SessionFile`] carries the UNION of what both crates need:
//! - `group_id` + `kind` — `report`'s parent/subagent grouping;
//! - `mtime` + `size` — `cost`'s [`filter_by_date_range`] date prefilter and cache-invalidation
//!   hash.
//!
//! Both `mtime` and `size` are read from the SAME `fs::metadata` call the empty-file check already
//! makes, so there is no extra stat per file.

use chrono::NaiveDate;
use eyre::{Result, bail};
use log::{debug, info, warn};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// Whether a discovered file is a top-level parent session JSONL or one of its subagent JSONLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    Parent,
    Subagent,
}

/// One discovered session JSONL file. The union of both consuming crates' needs (see the module
/// doc): `group_id`/`kind` drive `report`'s parent+subagent grouping; `mtime`/`size` drive `cost`'s
/// mtime date prefilter and cache-invalidation hash.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    /// The parent session's UUID stem. A parent file and its `subagents/*.jsonl` share this id, so
    /// subagent spend folds into the parent session's total.
    pub group_id: String,
    pub kind: SessionFileKind,
    pub mtime: SystemTime,
    pub size: u64,
}

const UUID_V4_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

fn uuid_v4_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(UUID_V4_PATTERN).expect("UUID-v4 pattern is a valid regex"))
}

/// Discover every session JSONL under the Claude projects directory: each top-level `*.jsonl` in
/// every project dir (a parent session) plus every `<session-uuid>/subagents/*.jsonl` (subagents
/// carrying the parent's session id).
///
/// Fail-loud (harvested from `report`): a top-level JSONL whose stem is not a UUID-v4, or a session
/// directory (one containing `subagents/`) whose name is not a UUID-v4, triggers [`bail!`] rather
/// than being misclassified. Real Claude Code session files are always UUID-v4 named, so a
/// non-UUID name is a corrupt/foreign layout that must surface loudly, never silently.
///
/// The returned list is sorted by path so the insertion order into any downstream parse/dedup
/// pipeline is stable across runs (`read_dir` yields entries in filesystem-dependent order, which
/// would otherwise make `cost`'s equal-cost dedup tie-break non-deterministic).
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

    // Sort by path so the insertion order into the parse/dedup pipeline is stable across runs.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    info!("scan::find_session_files: discovered {} files", files.len());
    Ok(files)
}

fn make_parent(path: PathBuf, stem: &str) -> Option<SessionFile> {
    let (mtime, size) = file_stat(&path)?;
    if size == 0 {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: stem.to_string(),
        kind: SessionFileKind::Parent,
        mtime,
        size,
    })
}

fn make_subagent(path: PathBuf, parent_stem: &str) -> Option<SessionFile> {
    let (mtime, size) = file_stat(&path)?;
    if size == 0 {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: parent_stem.to_string(),
        kind: SessionFileKind::Subagent,
        mtime,
        size,
    })
}

/// Read `(mtime, size)` from a single `fs::metadata` call. `None` (skip the file) on a metadata
/// error, which also covers the previous empty-file skip: a file we cannot stat is not counted.
fn file_stat(path: &Path) -> Option<(SystemTime, u64)> {
    match fs::metadata(path) {
        Ok(m) => Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())),
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

/// Prefilter session files by mtime as a *lower-bound optimization only*.
///
/// Counting is by entry timestamp (the counted-entry contract): a line counts iff its own
/// `timestamp` falls in the window, enforced per-entry by the consumer. This prefilter exists
/// solely to skip whole files that provably hold no in-window content, so it MUST NEVER drop a
/// file that could hold an in-window entry.
///
/// The only safe test is the lower bound `mtime_date >= start`. It is safe under the append-only
/// invariant: Claude Code only ever appends to a session JSONL, so a file's mtime is >= its newest
/// entry's timestamp. Therefore a file whose mtime falls before `start` has *every* entry before
/// `start` and cannot hold in-window content -- dropping it loses nothing.
///
/// There is deliberately NO upper bound (`mtime_date <= end`). A file touched after `end` (e.g. a
/// still-growing session queried for an earlier day) can still hold entries dated within the
/// window; a `<= end` exclusion would silently drop those in-window dollars.
pub fn filter_by_date_range(files: &[SessionFile], start: NaiveDate, end: NaiveDate) -> Vec<&SessionFile> {
    debug!(
        "scan::filter_by_date_range: start={}, end={}, input_count={}",
        start,
        end,
        files.len()
    );

    files
        .iter()
        .filter(|f| {
            let mtime: chrono::DateTime<chrono::Local> = f.mtime.into();
            let file_date = mtime.date_naive();
            // Lower bound only. Never exclude a file whose mtime is at/after `start`; the actual
            // window enforcement is the per-entry timestamp check in the consumer.
            file_date >= start
        })
        .collect()
}

/// The default Claude projects directory (`~/.claude/projects`).
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

#[cfg(test)]
mod tests;
