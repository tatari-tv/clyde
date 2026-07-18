//! Shared transcript-layout resolution: the (parent, subagents-dir) pair that `enrich` and the
//! MCP content tools (`session_grep`/`session_read`, Phases 6/7) both parse from.
//!
//! Promoted out of `enrich.rs` (Phase 5) so the resolution logic lives in exactly one place.

use std::path::{Path, PathBuf};

use crate::model::SessionRecord;

/// The (parent, subagents-dir) transcript layout for a record, resolved live-then-staged by
/// **existence** -- exactly like `session_open`'s 3-state resolution
/// (`mcp.rs`'s `open_result_for`): prefer the live transcript under `~/.claude/projects` if it
/// is still on disk (robust to a TTL reap between catalog lookup and use), else the Phase 1.5
/// staged copy if one exists, else `None` (nothing left to parse). Staged copies are plain jsonl
/// mirroring the live layout (`session::stage`), so one parse path serves both.
pub fn transcript_layout(rec: &SessionRecord) -> Option<(PathBuf, PathBuf)> {
    transcript_layout_parts(
        &rec.session_id,
        &rec.transcript_path,
        &rec.project_dir,
        rec.staged_path.as_deref(),
    )
}

/// The live-then-staged layout resolution over the raw fields, without a [`SessionRecord`]. The
/// `export` query maps its own columns (the enrichment fields `SessionRecord` omits) and reuses this
/// so the body-source fallback stays identical to `enrich` and the MCP content tools: prefer the
/// live `transcript_path` if on disk, else the staged copy, else `None`.
///
/// Both branches resolve by the existence of the actual `.jsonl` file, never a mere directory: the
/// staged branch requires `<staged>/<session-id>.jsonl` itself to be on disk, not just the staged
/// directory. A staged dir whose `.jsonl` was reaped therefore yields `None` (nothing to parse), so
/// `export` reports the contractually correct `body-error: "transcript missing"` rather than parsing
/// a nonexistent file to zero messages and reporting `"parsed empty"`.
pub fn transcript_layout_parts(
    session_id: &str,
    transcript_path: &Path,
    project_dir: &str,
    staged_path: Option<&Path>,
) -> Option<(PathBuf, PathBuf)> {
    if transcript_path.exists() {
        let subagents = Path::new(project_dir).join(session_id).join("subagents");
        return Some((transcript_path.to_path_buf(), subagents));
    }
    let staged = staged_path.filter(|p| p.exists())?;
    let parent = staged.join(format!("{session_id}.jsonl"));
    if !parent.exists() {
        return None;
    }
    let subagents = staged.join("subagents");
    Some((parent, subagents))
}

#[cfg(test)]
mod tests;
