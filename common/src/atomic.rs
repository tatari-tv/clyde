//! Atomic file writes, shared across every tool that mutates a settings/config file in place.
//!
//! [`write_atomic`] writes to a temp file created in the target's own parent directory (never the
//! OS temp dir, since a cross-filesystem rename fails outright), flushes it, then renames it over
//! the target. That closes the corruption window a plain `fs::write` leaves open: `fs::write`
//! truncates the target before writing its new bytes, so a crash or a torn write mid-copy can
//! leave the file empty or half-written. Mirrors the private `write_atomic` in
//! `clyde/src/bootstrap.rs`, generalized to take raw bytes and live in `common` so more than one
//! crate can share it.

use std::fs;
use std::io::Write;
use std::path::Path;

use eyre::{Context, Result};
use tempfile::NamedTempFile;

/// Atomically write `bytes` to `path`.
///
/// A temp file is created in `path`'s own parent directory (so the final rename never crosses a
/// filesystem boundary), written, flushed, then persisted (renamed) over `path`. If `path` already
/// exists, its file permissions are captured before the write and re-applied after the rename,
/// since `NamedTempFile` creates its temp file with its own default permissions, and a plain rename
/// would otherwise silently strip e.g. an existing executable bit (the same lesson
/// `clyde/src/bootstrap.rs::repoint_statusline` already learned). A read-only parent directory (or
/// any other create/write/rename failure) surfaces as a typed `eyre::Result` error, never a panic.
/// The directory to create the temp file in: `path`'s parent, with an EMPTY parent resolved to `.`.
///
/// **An empty parent is the current directory, not an absent one.** `Path::new("x").parent()` is
/// `Some("")` -- a bare relative filename, whose directory is `.` -- while only a root (`/`) has a
/// genuinely absent parent and yields `None`. Folding the two together with a
/// `filter(|p| !p.as_os_str().is_empty())` made [`write_atomic`] reject `x` outright, which is how
/// `report merge -o merged.json` broke when that module started delegating here: it computes its own
/// `.` fallback for `create_dir_all`, then passes the ORIGINAL path down, so the empty parent was
/// re-derived and errored.
///
/// The `None` arm is KEPT rather than folded into the same `.` default. It is the real "no parent"
/// case, and a blanket `unwrap_or(".")` would silently retarget a root write instead of failing.
///
/// A free function, and the reason is testability: the empty-parent case is only reachable through a
/// RELATIVE path, and asserting it end-to-end would mean mutating the process cwd -- global state
/// that would break any other test in this binary that resolves a relative path. Pure in, pure out,
/// so the distinction is pinned without touching the process.
///
/// `pub` for one caller: `report::merge::write_file_atomic` has to `create_dir_all` the output
/// directory before writing, and it must target the SAME directory [`write_atomic`] will resolve.
/// Computing that separately is how the rule ends up spelled twice and drifts.
pub fn parent_dir(path: &Path) -> Result<&Path> {
    match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(p) => Ok(p),
        None => Err(eyre::eyre!("path has no parent directory: {}", path.display())),
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    log::debug!("write_atomic: path={} bytes={}", path.display(), bytes.len());

    let parent = parent_dir(path)?;

    let existing_perms = match fs::metadata(path) {
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("write_atomic: failed to stat existing {}: {e}", path.display());
            return Err(e).with_context(|| format!("failed to stat {}", path.display()));
        }
    };

    let mut tmp =
        NamedTempFile::new_in(parent).with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    tmp.write_all(bytes)
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    tmp.flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;

    tmp.persist(path)
        .map_err(|e| eyre::eyre!("failed to rename temp file onto {}: {}", path.display(), e.error))?;

    if let Some(perms) = existing_perms {
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to restore permissions on {}", path.display()))?;
    }

    log::debug!("write_atomic: wrote {} bytes to {}", bytes.len(), path.display());
    Ok(())
}

#[cfg(test)]
mod tests;
