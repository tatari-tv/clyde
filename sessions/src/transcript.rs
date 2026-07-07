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
    if rec.transcript_path.exists() {
        let subagents = Path::new(&rec.project_dir).join(&rec.session_id).join("subagents");
        return Some((rec.transcript_path.clone(), subagents));
    }
    let staged = rec.staged_path.as_ref().filter(|p| p.exists())?;
    let parent = staged.join(format!("{}.jsonl", rec.session_id));
    let subagents = staged.join("subagents");
    Some((parent, subagents))
}

#[cfg(test)]
mod tests;
