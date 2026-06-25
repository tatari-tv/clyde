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

    info!("Found {} JSONL session files", files.len());
    Ok(files)
}

/// Filter session files to those modified within a date range (inclusive).
/// Uses file mtime as a heuristic - files modified on a given day likely contain
/// entries for that day.
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
            // Include files modified on or after start date
            // We can't perfectly filter by content date from mtime alone,
            // so we're inclusive - a file modified today may contain entries from yesterday
            file_date >= start && file_date <= end
        })
        .collect()
}

/// Get the default Claude projects directory
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_find_session_files() {
        let tmp = TempDir::new().expect("create temp dir");
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        // Create a JSONL file with content
        let jsonl_path = project_dir.join("session-123.jsonl");
        let mut file = fs::File::create(&jsonl_path).expect("create jsonl");
        writeln!(file, r#"{{"type":"system"}}"#).expect("write");

        // Create an empty file (should be skipped)
        fs::File::create(project_dir.join("empty.jsonl")).expect("create empty");

        // Create a non-JSONL file (should be skipped)
        let mut txt = fs::File::create(project_dir.join("notes.txt")).expect("create txt");
        writeln!(txt, "hello").expect("write");

        let files = find_session_files(tmp.path()).expect("find files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, jsonl_path);
    }

    #[test]
    fn test_find_session_files_empty_dir() {
        let tmp = TempDir::new().expect("create temp dir");
        let files = find_session_files(tmp.path()).expect("find files");
        assert!(files.is_empty());
    }

    #[test]
    fn test_find_session_files_nonexistent() {
        let files = find_session_files(Path::new("/nonexistent/path")).expect("find files");
        assert!(files.is_empty());
    }

    #[test]
    fn test_find_session_files_includes_subagents() {
        let tmp = TempDir::new().expect("create temp dir");
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        // Direct session JSONL
        let parent_jsonl = project_dir.join("abc123.jsonl");
        let mut f = fs::File::create(&parent_jsonl).expect("create parent jsonl");
        writeln!(f, r#"{{"type":"system"}}"#).expect("write");

        // Subagent JSONL at <uuid>/subagents/<agent>.jsonl
        let subagents_dir = project_dir.join("abc123").join("subagents");
        fs::create_dir_all(&subagents_dir).expect("create subagents dir");
        let agent_jsonl = subagents_dir.join("agent-aabbccdd.jsonl");
        let mut f = fs::File::create(&agent_jsonl).expect("create agent jsonl");
        writeln!(f, r#"{{"type":"assistant"}}"#).expect("write");

        // Empty subagent file (should be skipped)
        fs::File::create(subagents_dir.join("agent-empty.jsonl")).expect("create empty");

        let files = find_session_files(tmp.path()).expect("find files");
        assert_eq!(files.len(), 2);
        let paths: Vec<_> = files.iter().map(|f| &f.path).collect();
        assert!(paths.contains(&&parent_jsonl));
        assert!(paths.contains(&&agent_jsonl));
    }

    #[test]
    fn test_find_session_files_subagents_no_parent_jsonl() {
        let tmp = TempDir::new().expect("create temp dir");
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        // Subagent JSONL exists without a sibling parent .jsonl
        let subagents_dir = project_dir.join("orphan-uuid").join("subagents");
        fs::create_dir_all(&subagents_dir).expect("create subagents dir");
        let agent_jsonl = subagents_dir.join("agent-orphan.jsonl");
        let mut f = fs::File::create(&agent_jsonl).expect("create agent jsonl");
        writeln!(f, r#"{{"type":"assistant"}}"#).expect("write");

        let files = find_session_files(tmp.path()).expect("find files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, agent_jsonl);
    }

    #[test]
    fn test_find_session_files_tool_results_ignored() {
        let tmp = TempDir::new().expect("create temp dir");
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        // tool-results dir should not be traversed for JSONL
        let tool_results_dir = project_dir.join("abc123").join("tool-results");
        fs::create_dir_all(&tool_results_dir).expect("create tool-results dir");
        let mut f = fs::File::create(tool_results_dir.join("output.json")).expect("create json");
        writeln!(f, "{{}}").expect("write");

        // Only the subagents dir should be scanned; tool-results has no JSONL so result is 0
        let files = find_session_files(tmp.path()).expect("find files");
        assert!(files.is_empty());
    }
}
