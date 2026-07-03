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
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    log::debug!("write_atomic: path={} bytes={}", path.display(), bytes.len());

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| eyre::eyre!("path has no parent directory: {}", path.display()))?;

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
