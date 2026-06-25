//! Durable transcript staging (Phase 1.5): copy a session's live transcripts into a clyde-owned
//! location to beat Claude's 30-day TTL, decoupled from any knowledge-layer distillation.
//!
//! This reads only local files and writes only local files — no LLM, no vault, no work/personal
//! crossing. Copies are atomic (temp-in-dest + rename) and idempotent (a destination at least as
//! new as its source is left untouched), so re-staging a sweep is cheap and self-healing.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use log::{debug, trace, warn};

/// Result of staging one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// The session's staging directory (mirrors the parent + subagents layout).
    pub dir: PathBuf,
    /// Transcript files actually copied this call (0 when everything was already current).
    pub files_copied: usize,
    /// Total transcript files present for the session.
    pub files_total: usize,
}

/// Stage a session's transcripts from `project_dir` into `staged_root/<session_id>/`.
///
/// Stages the parent `<session_id>.jsonl` and every `<session_id>/subagents/*.jsonl`. A source
/// with no live parent (already TTL-reaped) still stages whatever subagents remain; callers should
/// avoid staging sessions already flagged archived. Returns the staging dir and copy counts.
pub fn stage_session(project_dir: &Path, session_id: &str, staged_root: &Path) -> Result<Staged> {
    debug!(
        "stage::stage_session: session_id={} project_dir={} staged_root={}",
        session_id,
        project_dir.display(),
        staged_root.display()
    );
    let dest_dir = staged_root.join(session_id);

    let mut files_total = 0usize;
    let mut files_copied = 0usize;

    let parent_src = project_dir.join(format!("{session_id}.jsonl"));
    if parent_src.is_file() {
        files_total += 1;
        let parent_dst = dest_dir.join(format!("{session_id}.jsonl"));
        if copy_if_newer(&parent_src, &parent_dst)? {
            files_copied += 1;
        }
    }

    let subagents_src = project_dir.join(session_id).join("subagents");
    if subagents_src.is_dir() {
        for entry in
            fs::read_dir(&subagents_src).with_context(|| format!("failed to read {}", subagents_src.display()))?
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("stage: error reading subagent entry: {e}");
                    continue;
                }
            };
            let src = entry.path();
            if !src.is_file() || src.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(name) = src.file_name() else { continue };
            files_total += 1;
            let dst = dest_dir.join("subagents").join(name);
            if copy_if_newer(&src, &dst)? {
                files_copied += 1;
            }
        }
    }

    debug!(
        "stage::stage_session: session_id={} files_total={} files_copied={}",
        session_id, files_total, files_copied
    );
    Ok(Staged {
        dir: dest_dir,
        files_copied,
        files_total,
    })
}

/// Atomically copy `src` to `dst` unless `dst` is already at least as new as `src`.
/// Returns `true` when a copy happened.
fn copy_if_newer(src: &Path, dst: &Path) -> Result<bool> {
    if let (Some(s), Some(d)) = (mtime(src), mtime(dst))
        && d >= s
    {
        trace!("stage: {} is current, skipping", dst.display());
        return Ok(false);
    }
    let parent = dst
        .parent()
        .ok_or_else(|| eyre::eyre!("staged destination {} has no parent", dst.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create staged dir {}", parent.display()))?;

    // Atomic: write to a temp file in the destination dir, then rename over the target.
    let bytes = fs::read(src).with_context(|| format!("failed to read {}", src.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    {
        use std::io::Write;
        tmp.write_all(&bytes).context("failed to write staged temp")?;
        tmp.flush().context("failed to flush staged temp")?;
    }
    tmp.persist(dst)
        .with_context(|| format!("failed to persist staged copy to {}", dst.display()))?;
    Ok(true)
}

fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .ok()
}

#[cfg(test)]
mod tests;
