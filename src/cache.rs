use chrono::NaiveDate;
use eyre::{Context, Result};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::scanner::SessionFile;

const CACHE_VERSION: u64 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedDay {
    pub cost: f64,
    pub sessions: usize,
    pub mtime_hash: u64,
    #[serde(default)]
    pub version: u64,
}

/// Compute a hash of file paths, mtimes, and sizes for cache invalidation
pub fn compute_mtime_hash(files: &[&SessionFile]) -> u64 {
    trace!("compute_mtime_hash: file_count={}", files.len());

    let mut hasher = DefaultHasher::new();
    for f in files {
        f.path.to_string_lossy().hash(&mut hasher);
        let mtime_secs = f
            .mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        mtime_secs.hash(&mut hasher);
        f.size.hash(&mut hasher);
    }
    hasher.finish()
}

/// Get the cache directory (~/.cache/ccu/)
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("ccu"))
}

/// Try to load a cached day summary
pub fn load_cached_day(date: NaiveDate, mtime_hash: u64) -> Option<CachedDay> {
    let dir = cache_dir()?;
    let path = dir.join(format!("{}.json", date));

    let content = fs::read_to_string(&path).ok()?;
    let cached: CachedDay = serde_json::from_str(&content).ok()?;

    if cached.version != CACHE_VERSION {
        info!(
            "Cache miss for {} (version mismatch: {} != {})",
            date, cached.version, CACHE_VERSION
        );
        None
    } else if cached.mtime_hash == mtime_hash {
        info!("Cache hit for {}", date);
        Some(cached)
    } else {
        info!("Cache miss for {} (hash mismatch)", date);
        None
    }
}

/// Save a day summary to the cache
pub fn save_cached_day(date: NaiveDate, cost: f64, sessions: usize, mtime_hash: u64) -> Result<()> {
    let dir = match cache_dir() {
        Some(d) => d,
        None => return Ok(()),
    };

    fs::create_dir_all(&dir).context("Failed to create cache directory")?;

    let cached = CachedDay {
        cost,
        sessions,
        mtime_hash,
        version: CACHE_VERSION,
    };

    let path = dir.join(format!("{}.json", date));
    let content = serde_json::to_string(&cached).context("Failed to serialize cache")?;
    fs::write(&path, content).context("Failed to write cache file")?;

    trace!("Cached day {} to {}", date, path.display());
    Ok(())
}

/// Remove stale cache entries older than the given number of days
pub fn prune_cache(keep_days: u32) -> Result<()> {
    trace!("prune_cache: keep_days={}", keep_days);

    let dir = match cache_dir() {
        Some(d) => d,
        None => return Ok(()),
    };

    if !dir.exists() {
        return Ok(());
    }

    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(i64::from(keep_days));

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(date) = stem.parse::<NaiveDate>()
            && date < cutoff
            && let Err(e) = fs::remove_file(&path)
        {
            warn!("Failed to prune cache file {}: {}", path.display(), e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::SessionFile;
    use std::time::SystemTime;

    #[test]
    fn test_compute_mtime_hash_deterministic() {
        let files = [SessionFile {
            path: PathBuf::from("/tmp/test.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1024,
        }];
        let refs: Vec<&SessionFile> = files.iter().collect();
        let h1 = compute_mtime_hash(&refs);
        let h2 = compute_mtime_hash(&refs);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_mtime_hash_changes_with_size() {
        let f1 = [SessionFile {
            path: PathBuf::from("/tmp/test.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1024,
        }];
        let f2 = [SessionFile {
            path: PathBuf::from("/tmp/test.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 2048,
        }];
        let r1: Vec<&SessionFile> = f1.iter().collect();
        let r2: Vec<&SessionFile> = f2.iter().collect();
        assert_ne!(compute_mtime_hash(&r1), compute_mtime_hash(&r2));
    }

    #[test]
    fn test_load_cached_day_miss() {
        let result = load_cached_day(NaiveDate::from_ymd_opt(1900, 1, 1).expect("valid date"), 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_load_cached_day() {
        let date = NaiveDate::from_ymd_opt(2099, 12, 31).expect("valid date");
        let hash = 42;

        save_cached_day(date, 14.23, 3, hash).expect("save");

        let loaded = load_cached_day(date, hash);
        assert!(loaded.is_some());
        let cached = loaded.expect("should be Some");
        assert!((cached.cost - 14.23).abs() < f64::EPSILON);
        assert_eq!(cached.sessions, 3);

        let loaded = load_cached_day(date, 999);
        assert!(loaded.is_none());

        // Cleanup
        if let Some(dir) = cache_dir() {
            let _ = fs::remove_file(dir.join(format!("{}.json", date)));
        }
    }

    #[test]
    fn test_compute_mtime_hash_changes_with_mtime() {
        let f1 = [SessionFile {
            path: PathBuf::from("/tmp/test.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1024,
        }];
        let f2 = [SessionFile {
            path: PathBuf::from("/tmp/test.jsonl"),
            mtime: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60),
            size: 1024,
        }];
        let r1: Vec<&SessionFile> = f1.iter().collect();
        let r2: Vec<&SessionFile> = f2.iter().collect();
        assert_ne!(compute_mtime_hash(&r1), compute_mtime_hash(&r2));
    }

    #[test]
    fn test_compute_mtime_hash_changes_with_path() {
        let f1 = [SessionFile {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1024,
        }];
        let f2 = [SessionFile {
            path: PathBuf::from("/tmp/b.jsonl"),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1024,
        }];
        let r1: Vec<&SessionFile> = f1.iter().collect();
        let r2: Vec<&SessionFile> = f2.iter().collect();
        assert_ne!(compute_mtime_hash(&r1), compute_mtime_hash(&r2));
    }

    #[test]
    fn test_compute_mtime_hash_empty() {
        let refs: Vec<&SessionFile> = vec![];
        let h1 = compute_mtime_hash(&refs);
        let h2 = compute_mtime_hash(&refs);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_load_cached_day_corrupt_json() {
        let dir = cache_dir().expect("cache dir");
        fs::create_dir_all(&dir).expect("create cache dir");

        let date = NaiveDate::from_ymd_opt(2098, 1, 1).expect("valid date");
        let path = dir.join(format!("{}.json", date));
        fs::write(&path, "not valid json {{{").expect("write corrupt file");

        let result = load_cached_day(date, 0);
        assert!(result.is_none());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_cache_removes_old_entries() {
        let dir = cache_dir().expect("cache dir");
        fs::create_dir_all(&dir).expect("create cache dir");

        // Create a cache file for a date well in the past (200 days ago)
        let old_date = chrono::Local::now().date_naive() - chrono::Duration::days(200);
        let old_path = dir.join(format!("{}.json", old_date));
        let cached = CachedDay {
            cost: 1.0,
            sessions: 1,
            mtime_hash: 0,
            version: CACHE_VERSION,
        };
        fs::write(&old_path, serde_json::to_string(&cached).expect("serialize")).expect("write");
        assert!(old_path.exists());

        // Create a cache file for today (should be kept)
        let today = chrono::Local::now().date_naive();
        let today_path = dir.join(format!("{}.json", today));
        fs::write(&today_path, serde_json::to_string(&cached).expect("serialize")).expect("write");

        prune_cache(90).expect("prune");

        assert!(!old_path.exists(), "old cache file should be pruned");
        assert!(today_path.exists(), "today's cache file should be kept");

        let _ = fs::remove_file(&today_path);
    }

    #[test]
    fn test_prune_cache_ignores_non_json_files() {
        let dir = cache_dir().expect("cache dir");
        fs::create_dir_all(&dir).expect("create cache dir");

        let non_json = dir.join("notes.txt");
        fs::write(&non_json, "keep me").expect("write");

        prune_cache(0).expect("prune with 0 days should not error");

        assert!(non_json.exists(), "non-json file should be untouched");

        let _ = fs::remove_file(&non_json);
    }

    #[test]
    fn test_prune_cache_nonexistent_dir() {
        // prune_cache should succeed gracefully when cache dir doesn't exist
        // We can't easily force cache_dir() to return a nonexistent path,
        // but we can verify it doesn't error when the dir exists but is empty
        let dir = cache_dir().expect("cache dir");
        fs::create_dir_all(&dir).expect("create cache dir");
        prune_cache(90).expect("prune empty dir should succeed");
    }
}
