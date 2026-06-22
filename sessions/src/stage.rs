//! Staging sweep (Phase 1.5): durably copy dormant sessions' transcripts before Claude's 30-day
//! TTL reaps them, and record the staged location in `sessions.db`. Decoupled from distillation —
//! this is pure local file insurance, committing to none of the knowledge-layer questions.

use std::path::Path;

use chrono::{DateTime, Utc};
use eyre::Result;
use log::{debug, info, warn};

use crate::db::Db;
use crate::model::StageStats;

/// Stage every dormant (non-archived) session. With `dormant_before = None`, stages all
/// non-archived sessions; otherwise only those last modified at or before the cutoff.
pub fn stage_dormant(db: &Db, dormant_before: Option<DateTime<Utc>>, staged_root: &Path) -> Result<StageStats> {
    debug!(
        "stage::stage_dormant: dormant_before={:?} staged_root={}",
        dormant_before,
        staged_root.display()
    );
    let candidates = db.staging_candidates(dormant_before)?;
    let mut stats = StageStats {
        considered: candidates.len(),
        ..Default::default()
    };

    for rec in &candidates {
        let project_dir = Path::new(&rec.project_dir);
        let staged = match session::stage::stage_session(project_dir, &rec.session_id, staged_root) {
            Ok(s) => s,
            Err(e) => {
                warn!("stage_dormant: failed to stage {}: {e}", rec.session_id);
                continue;
            }
        };
        if staged.files_total == 0 {
            warn!(
                "stage_dormant: no live transcript files for {}; skipping",
                rec.session_id
            );
            continue;
        }
        db.set_staged_path(&rec.session_id, &staged.dir)?;
        stats.files_copied += staged.files_copied;
        if staged.files_copied > 0 {
            stats.staged += 1;
        } else {
            stats.up_to_date += 1;
        }
    }

    info!(
        "stage::stage_dormant: considered={} staged={} up_to_date={} files_copied={}",
        stats.considered, stats.staged, stats.up_to_date, stats.files_copied
    );
    Ok(stats)
}

#[cfg(test)]
mod tests;
