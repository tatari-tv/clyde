//! Reindex: scan `~/.claude/projects`, parse, and upsert into `sessions.db`.
//!
//! Incremental by parent-transcript mtime — unchanged sessions are skipped. After upserting,
//! a reconcile pass flags rows whose transcripts have been TTL-reaped as `archived`.
//!
//! Repo attribution (schema v10, `docs/design/2026-07-26-report-story-fidelity.md`) resolves on
//! EVERY session on EVERY pass, independent of the content-mtime skip: a session's repo confidence
//! can change with no transcript change at all (its cwd is deleted, or re-cloned as a different
//! repo), so it cannot be gated behind the same skip key as the content columns.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Utc;
use common::repo::{RepoSource, Resolver};
use eyre::Result;
use log::{debug, info};
use session::{parse, scan};

use crate::db::{Db, Upsert};
use crate::model::ReindexStats;

/// Run a full incremental reindex against `projects_dir`, writing into `db`. `repo_root` is the
/// configured clone root rule 4 (`common::repo::from_path_guess`) matches against — `clyde.yml`'s
/// `repo-root`, default `<home>/repos`.
pub fn reindex(db: &Db, projects_dir: &Path, repo_root: &Path) -> Result<ReindexStats> {
    debug!(
        "index::reindex: projects_dir={} repo_root={}",
        projects_dir.display(),
        repo_root.display()
    );
    let files = scan::find_session_files(projects_dir)?;
    let sessions = parse::parse_sessions(&files);
    let host = gethostname::gethostname().to_string_lossy().into_owned();

    let mut stats = ReindexStats {
        scanned: sessions.len(),
        ..Default::default()
    };
    let mut resolver = Resolver::new();
    // Rule 3 (files-touched) is Phase 3's `repos_touched`; every session resolves against an empty
    // map here, so the chain degrades to rules 1/2/4 only until that lands.
    let repos_touched: BTreeMap<String, u64> = BTreeMap::new();
    for parsed in &sessions {
        match db.upsert_session(parsed, &host)? {
            Upsert::Inserted | Upsert::Updated => stats.upserted += 1,
            Upsert::SkippedUnchanged => stats.skipped_unchanged += 1,
        }
        if let Some(cwd) = &parsed.cwd
            && let Some(resolved) = resolver.resolve(cwd, db, &repos_touched, repo_root)
        {
            db.upsert_repo(&parsed.session_id, &resolved)?;
            // Every rule-1 (GitOrigin) success is fresh live evidence for rule 2's learned map:
            // latest-observation-wins, so a path deleted and re-cloned as a different repo
            // self-corrects (see the design doc's "opposite write policies").
            if resolved.source == RepoSource::GitOrigin {
                db.record_repo_path(&cwd.to_string_lossy(), &resolved.repo, Utc::now())?;
            }
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
