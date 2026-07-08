use chrono::NaiveDate;
use eyre::{Context, Result};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::scanner::SessionFile;

const CACHE_VERSION: u64 = 5;

/// FNV-1a 64-bit offset basis. Pinned so cache keys are stable across Rust releases, unlike
/// `DefaultHasher` (SipHash), whose output is not guaranteed stable toolchain-to-toolchain.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a over a single byte slice, folding into the running hash state.
fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedDay {
    pub cost: f64,
    pub sessions: usize,
    pub mtime_hash: u64,
    #[serde(default)]
    pub version: u64,
}

/// Compute a hash of file paths, mtimes, and sizes for cache invalidation. Uses inline
/// FNV-1a (not `DefaultHasher`/SipHash) so the hash is stable across Rust toolchain versions,
/// since the value is persisted to disk in the cache file.
pub fn compute_mtime_hash(files: &[&SessionFile]) -> u64 {
    trace!("compute_mtime_hash: file_count={}", files.len());

    let mut hash = FNV_OFFSET_BASIS;
    for f in files {
        hash = fnv1a_update(hash, f.path.to_string_lossy().as_bytes());
        // Nanosecond precision (not whole seconds): two writes to the same file within one
        // wall-clock second would otherwise hash identically and the cache could serve stale data.
        // `size` already changes on the common append-only path, but nanoseconds close the
        // same-second-same-size window where the filesystem records sub-second mtime. `as_nanos()`
        // is a fixed-width integer, so the hash stays stable across toolchains.
        let mtime_nanos = f
            .mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        hash = fnv1a_update(hash, &mtime_nanos.to_le_bytes());
        hash = fnv1a_update(hash, &f.size.to_le_bytes());
    }
    hash
}

/// Get the cache directory (~/.cache/clyde/cost/). Disposable day-cost cache: not migrated by
/// bootstrap, it rebuilds at this clyde-namespaced path on first run (was ~/.cache/ccu/).
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("clyde").join("cost"))
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
    use common::SessionFileKind;
    use std::time::SystemTime;

    /// Build a `SessionFile` for the cache-hash tests. Only `path`, `mtime`, and `size` feed
    /// `compute_mtime_hash` (grouping is irrelevant to cache invalidation), so `group_id`/`kind`
    /// carry fixed placeholder values — this keeps the unified 5-field type from bloating every
    /// literal in this module.
    fn sf(path: &str, mtime: SystemTime, size: u64) -> SessionFile {
        SessionFile {
            path: PathBuf::from(path),
            group_id: String::new(),
            kind: SessionFileKind::Parent,
            mtime,
            size,
        }
    }

    /// Pins `compute_mtime_hash` against a known `(path, mtime, size)` tuple so an
    /// accidental algorithm change (e.g. reverting to `DefaultHasher`, or swapping the
    /// FNV-1a constants) is caught even though the other tests here only assert
    /// determinism/uniqueness, not a specific value.
    #[test]
    fn test_compute_mtime_hash_pinned_vector() {
        let files = [sf(
            "/tmp/pinned.jsonl",
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            4096,
        )];
        let refs: Vec<&SessionFile> = files.iter().collect();
        assert_eq!(compute_mtime_hash(&refs), 0x9207_5a54_a049_57ce);
    }

    #[test]
    fn test_compute_mtime_hash_deterministic() {
        let files = [sf("/tmp/test.jsonl", SystemTime::UNIX_EPOCH, 1024)];
        let refs: Vec<&SessionFile> = files.iter().collect();
        let h1 = compute_mtime_hash(&refs);
        let h2 = compute_mtime_hash(&refs);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_mtime_hash_changes_with_size() {
        let f1 = [sf("/tmp/test.jsonl", SystemTime::UNIX_EPOCH, 1024)];
        let f2 = [sf("/tmp/test.jsonl", SystemTime::UNIX_EPOCH, 2048)];
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
        let f1 = [sf("/tmp/test.jsonl", SystemTime::UNIX_EPOCH, 1024)];
        let f2 = [sf(
            "/tmp/test.jsonl",
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60),
            1024,
        )];
        let r1: Vec<&SessionFile> = f1.iter().collect();
        let r2: Vec<&SessionFile> = f2.iter().collect();
        assert_ne!(compute_mtime_hash(&r1), compute_mtime_hash(&r2));
    }

    #[test]
    fn test_compute_mtime_hash_changes_with_path() {
        let f1 = [sf("/tmp/a.jsonl", SystemTime::UNIX_EPOCH, 1024)];
        let f2 = [sf("/tmp/b.jsonl", SystemTime::UNIX_EPOCH, 1024)];
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
