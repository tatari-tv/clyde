//! Reindex: scan `~/.claude/projects`, parse, and upsert into `sessions.db`.
//!
//! Incremental by parent-transcript mtime — unchanged sessions are skipped. After upserting,
//! a reconcile pass flags rows whose transcripts have been TTL-reaped as `archived`.

use std::path::Path;

use eyre::Result;
use log::{debug, info};
use session::{parse, scan};

use crate::db::{Db, Upsert};
use crate::model::ReindexStats;

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

#[cfg(test)]
mod tests;
