//! Reindex: scan `~/.claude/projects`, parse, and upsert into `sessions.db`.
//!
//! Incremental by parent-transcript mtime -- unchanged sessions are skipped. After upserting,
//! a reconcile pass flags rows whose transcripts have been TTL-reaped as `archived`.
//!
//! Repo attribution (schema v10, `docs/design/2026-07-26-report-story-fidelity.md`) resolves on
//! EVERY session on EVERY pass, independent of the content-mtime skip: a session's repo confidence
//! can change with no transcript change at all (its cwd is deleted, or re-cloned as a different
//! repo), so it cannot be gated behind the same skip key as the content columns.
//!
//! Rule 3 (`files-touched`) needs an input this pass cannot have yet. `Outcomes::repos_touched` is
//! written by `efficiency::reindex_efficiency`, which runs AFTER the content reindex, so a session
//! reindexed for the first time has a NULL `outcome_json` while [`reindex`] is running. That is what
//! [`resolve_repos`] is for: the caller runs it once the efficiency pass has written the blobs, and
//! the chain converges within a single `clyde session reindex` instead of needing a second one.

use std::path::Path;

use chrono::Utc;
use common::repo::{RepoSource, Resolver};
use eyre::Result;
use log::{debug, info};
use session::{parse, scan};

use crate::db::{Db, Upsert};
use crate::model::ReindexStats;

/// Rule 3's rank. A session already resolved at this rank or better cannot be improved by the
/// post-efficiency [`resolve_repos`] pass (the write is upgrade-only), so it is skipped there.
const FILES_TOUCHED_RANK: i64 = RepoSource::FilesTouched.rank();

/// Run a full incremental reindex against `projects_dir`, writing into `db`. `repo_root` is the
/// configured clone root rule 4 (`common::repo::from_path_guess`) matches against -- `clyde.yml`'s
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
    for parsed in &sessions {
        match db.upsert_session(parsed, &host)? {
            Upsert::Inserted | Upsert::Updated => stats.upserted += 1,
            Upsert::SkippedUnchanged => stats.skipped_unchanged += 1,
        }
        if let Some(cwd) = &parsed.cwd {
            // Rule 3's input comes from the PERSISTED `outcome_json` (empty for a session the
            // efficiency pass has not reached yet); `resolve_repos` closes that gap after it has.
            let repos_touched = db.repos_touched(&parsed.session_id)?;
            apply_chain(db, &mut resolver, &parsed.session_id, cwd, &repos_touched, repo_root)?;
        }
    }
    stats.archived = db.reconcile_archived()?;
    info!(
        "index::reindex: scanned={} upserted={} skipped={} archived={}",
        stats.scanned, stats.upserted, stats.skipped_unchanged, stats.archived
    );
    Ok(stats)
}

/// Re-run the repo chain over the catalog for every session it could still improve, reading rule 3's
/// `repos_touched` from the now-written `outcome_json`. Returns the number of sessions evaluated.
///
/// Catalog-driven, so it costs no transcript scan and no parse: the cwd and the outcome blob are
/// both columns. Scoped to `repo_rank > files-touched`, so a session already resolved by rule 1 or 2
/// is not re-evaluated for an answer that could not win anyway.
///
/// Call this AFTER `efficiency::reindex_efficiency`. Calling it before is harmless but pointless --
/// every `repos_touched` would still be empty and rule 3 would abstain everywhere.
pub fn resolve_repos(db: &Db, repo_root: &Path) -> Result<usize> {
    debug!("index::resolve_repos: repo_root={}", repo_root.display());
    let candidates = db.repo_candidates(FILES_TOUCHED_RANK)?;
    let mut resolver = Resolver::new();
    for candidate in &candidates {
        apply_chain(
            db,
            &mut resolver,
            &candidate.session_id,
            Path::new(&candidate.cwd),
            &candidate.repos_touched,
            repo_root,
        )?;
    }
    info!(
        "index::resolve_repos: evaluated {} unresolved session(s)",
        candidates.len()
    );
    Ok(candidates.len())
}

/// Run the four-rule chain for one session and persist what it found: the upgrade-only
/// `sessions.repo` write, plus the learned-path record on a rule-1 hit.
///
/// Shared by [`reindex`] and [`resolve_repos`] so the two passes can never diverge on WHICH rules
/// run or on what a hit records -- the only difference between them is where `repos_touched` and the
/// cwd came from.
fn apply_chain(
    db: &Db,
    resolver: &mut Resolver,
    session_id: &str,
    cwd: &Path,
    repos_touched: &std::collections::BTreeMap<String, u64>,
    repo_root: &Path,
) -> Result<()> {
    let Some(resolved) = resolver.resolve(cwd, db, repos_touched, repo_root) else {
        return Ok(());
    };
    db.upsert_repo(session_id, &resolved)?;
    // Every rule-1 (GitOrigin) success is fresh live evidence for rule 2's learned map:
    // latest-observation-wins, so a path deleted and re-cloned as a different repo self-corrects
    // (see the design doc's "opposite write policies").
    if resolved.source == RepoSource::GitOrigin {
        db.record_repo_path(&cwd.to_string_lossy(), &resolved.repo, Utc::now())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
