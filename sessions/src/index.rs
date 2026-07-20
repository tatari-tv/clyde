//! Reindex: scan `~/.claude/projects`, parse, and upsert into `sessions.db`.
//!
//! Incremental by parent-transcript mtime — unchanged sessions are skipped. After upserting,
//! a reconcile pass flags rows whose transcripts have been TTL-reaped as `archived`.

use std::path::Path;

use eyre::{Context, Result};
use log::{debug, info, warn};
use session::{parse, scan};

use crate::db::{Db, Upsert};
use crate::model::{ReindexStats, ReparseStats, SessionRecord};
use crate::transcript::transcript_layout;

/// Run a full incremental reindex against `projects_dir`, writing into `db`.
pub fn reindex(db: &Db, projects_dir: &Path) -> Result<ReindexStats> {
    debug!("index::reindex: projects_dir={}", projects_dir.display());
    let files = scan::find_session_files(projects_dir)?;
    let sessions = parse::parse_sessions(&files);
    let host = gethostname::gethostname().to_string_lossy().into_owned();

    let mut stats = ReindexStats {
        scanned: sessions.len(),
        ..Default::default()
    };
    for parsed in &sessions {
        match db.upsert_session(parsed, &host)? {
            Upsert::Inserted | Upsert::Updated => stats.upserted += 1,
            Upsert::SkippedUnchanged => stats.skipped_unchanged += 1,
        }
    }
    stats.archived = db.reconcile_archived()?;
    info!(
        "index::reindex: scanned={} upserted={} skipped={} archived={}",
        stats.scanned, stats.upserted, stats.skipped_unchanged, stats.archived
    );
    Ok(stats)
}

/// Backfill the v6 `files_touched` column on existing rows via two passes under `reindex --reparse`:
///
/// 1. **Live pass:** re-parse every transcript the scan finds with the unchanged-mtime skip defeated
///    ([`Db::reparse_session`]), so a live row's `files_touched` is (re)written even though its mtime
///    has not changed. This reaches only rows whose transcript is still under `~/.claude/projects`.
/// 2. **Staged pass:** for rows still `files_touched IS NULL AND staged_path IS NOT NULL`, parse the
///    durable staged copy through the SAME extraction path ([`parse::parse_one`]) and write ONLY the
///    new column via the narrow [`Db::set_files_touched`] — never [`Db::upsert_session`], which would
///    clobber `modified`/`transcript_path`.
///
/// Per-row failures are skipped-and-logged (a bad transcript must not strand the rest of the batch);
/// [`ReparseStats::failed`] counts them so the caller can exit nonzero. Rows with neither a reachable
/// live nor a parseable staged transcript stay NULL ("unknowable"), counted as `staged_skipped`.
pub fn reparse(db: &Db, projects_dir: &Path) -> Result<ReparseStats> {
    debug!("index::reparse: projects_dir={}", projects_dir.display());
    let mut stats = ReparseStats::default();

    // --- Live pass: force-reparse every scanned session (defeat the unchanged-mtime skip). ---
    let files = scan::find_session_files(projects_dir)?;
    let sessions = parse::parse_sessions(&files);
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    stats.live_scanned = sessions.len();
    for parsed in &sessions {
        match db.reparse_session(parsed, &host) {
            Ok(_) => stats.live_populated += 1,
            Err(e) => {
                warn!("index::reparse: live re-parse failed for {}: {e:#}", parsed.session_id);
                stats.failed += 1;
            }
        }
    }
    // Mirror `reindex`: flag rows whose live transcript is gone as archived (best-effort — a
    // reconcile failure must not abort the staged pass that follows).
    if let Err(e) = db.reconcile_archived() {
        warn!("index::reparse: reconcile_archived failed: {e:#}");
    }

    // --- Staged pass: fill rows the scan cannot reach from their durable staged copy. ---
    let candidates = db.files_touched_backfill_candidates()?;
    stats.staged_candidates = candidates.len();
    for rec in &candidates {
        match backfill_staged(db, rec) {
            Ok(true) => stats.staged_populated += 1,
            Ok(false) => stats.staged_skipped += 1,
            Err(e) => {
                warn!("index::reparse: staged backfill failed for {}: {e:#}", rec.session_id);
                stats.failed += 1;
            }
        }
    }

    info!(
        "index::reparse: live_scanned={} live_populated={} staged_candidates={} staged_populated={} staged_skipped={} failed={}",
        stats.live_scanned,
        stats.live_populated,
        stats.staged_candidates,
        stats.staged_populated,
        stats.staged_skipped,
        stats.failed,
    );
    Ok(stats)
}

/// Parse one staged-pass candidate through the single extraction path and persist ONLY its
/// `files_touched` via the narrow writer. Returns `Ok(true)` when the column was written, `Ok(false)`
/// when the transcript is unreachable/unparseable (the row stays NULL — "unknowable", not a failure),
/// and `Err` only on a real DB/serialization fault (which the caller counts as a per-row failure).
fn backfill_staged(db: &Db, rec: &SessionRecord) -> Result<bool> {
    debug!("index::backfill_staged: session_id={}", rec.session_id);
    let Some((parent, subagents)) = transcript_layout(rec) else {
        debug!(
            "index::backfill_staged: {} has no reachable transcript; leaving files_touched NULL",
            rec.session_id
        );
        return Ok(false);
    };
    let Some(parsed) = parse::parse_one(&rec.session_id, &parent, &subagents) else {
        debug!(
            "index::backfill_staged: {} parsed to no session; leaving files_touched NULL",
            rec.session_id
        );
        return Ok(false);
    };
    let json = serde_json::to_string(&parsed.files_touched).context("serialize files_touched to a JSON array")?;
    let updated = db.set_files_touched(&rec.session_id, &json)?;
    Ok(updated)
}

#[cfg(test)]
mod tests;
