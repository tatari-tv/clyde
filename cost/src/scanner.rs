use chrono::NaiveDate;
use eyre::{Context, Result};
use log::{debug, info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub size: u64,
}

/// Collect a single JSONL file into the files list, skipping empty files.
fn collect_jsonl_file(path: &Path, files: &mut Vec<SessionFile>) {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warn!("Error reading metadata for {}: {}", path.display(), e);
            return;
        }
    };
    if metadata.len() == 0 {
        return;
    }
    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    files.push(SessionFile {
        path: path.to_path_buf(),
        mtime,
        size: metadata.len(),
    });
}

/// Discover all JSONL session files under the Claude projects directory
pub fn find_session_files(projects_dir: &Path) -> Result<Vec<SessionFile>> {
    debug!("find_session_files: projects_dir={}", projects_dir.display());

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    let project_dirs = fs::read_dir(projects_dir).context("Failed to read Claude projects directory")?;

    for entry in project_dirs {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("Error reading directory entry: {}", e);
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Error reading project dir {}: {}", path.display(), e);
                continue;
            }
        };

        for file_entry in dir_entries {
            let file_entry = match file_entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("Error reading file entry: {}", e);
                    continue;
                }
            };

            let file_path = file_entry.path();

            // Direct .jsonl file in the project dir
            if file_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                collect_jsonl_file(&file_path, &mut files);
                continue;
            }

            // Session UUID directory: look for subagents/*.jsonl inside it
            if file_path.is_dir() {
                let subagents_dir = file_path.join("subagents");
                if subagents_dir.is_dir() {
                    let sub_entries = match fs::read_dir(&subagents_dir) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Error reading subagents dir {}: {}", subagents_dir.display(), e);
                            continue;
                        }
                    };
                    for sub_entry in sub_entries {
                        let sub_entry = match sub_entry {
                            Ok(e) => e,
                            Err(e) => {
                                warn!("Error reading subagent entry: {}", e);
                                continue;
                            }
                        };
                        let sub_path = sub_entry.path();
                        if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                            collect_jsonl_file(&sub_path, &mut files);
                        }
                    }
                }
            }
        }
    }

    // Sort by path so the insertion order into the parse/dedup pipeline is stable across runs.
    // `read_dir` yields entries in filesystem-dependent order, which would otherwise make the
    // equal-cost dedup tie-break (and thus per-session cost attribution) non-deterministic.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    info!("Found {} JSONL session files", files.len());
    Ok(files)
}

/// Prefilter session files by mtime as a *lower-bound optimization only*.
///
/// Counting is by entry timestamp (the counted-entry contract): a line counts iff its own
/// `timestamp` falls in the window, enforced per-entry in `compute_summaries`. This prefilter
/// exists solely to skip whole files that provably hold no in-window content, so it MUST NEVER
/// drop a file that could hold an in-window entry.
///
/// The only safe test is the lower bound `mtime_date >= start`. It is safe under the append-only
/// invariant: Claude Code only ever appends to a session JSONL, so a file's mtime is >= its newest
/// entry's timestamp. Therefore a file whose mtime falls before `start` has *every* entry before
/// `start` and cannot hold in-window content -- dropping it loses nothing.
///
/// There is deliberately NO upper bound (`mtime_date <= end`). A file touched after `end` (e.g. a
/// still-growing session queried for an earlier day) can still hold entries dated within the
/// window; the old `<= end` exclusion silently dropped those in-window dollars. Removing it is the
/// Phase 1 correctness fix.
pub fn filter_by_date_range(files: &[SessionFile], start: NaiveDate, end: NaiveDate) -> Vec<&SessionFile> {
    debug!(
        "filter_by_date_range: start={}, end={}, input_count={}",
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
            // window enforcement is the per-entry timestamp check in compute_summaries.
            file_date >= start
        })
        .collect()
}

/// Get the default Claude projects directory
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

#[cfg(test)]
mod tests;
