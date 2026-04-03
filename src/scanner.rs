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
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let metadata = match fs::metadata(&file_path) {
                Ok(m) => m,
                Err(e) => {
                    warn!("Error reading metadata for {}: {}", file_path.display(), e);
                    continue;
                }
            };

            if metadata.len() == 0 {
                continue;
            }

            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            files.push(SessionFile {
                path: file_path,
                mtime,
                size: metadata.len(),
            });
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
}
