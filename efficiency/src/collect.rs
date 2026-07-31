//! Discovery + per-session efficiency computation: scans the Claude projects tree via
//! `common::scan`, groups files by session (`group_id`), extracts + folds + scores each group.
//! This is the seam every `clyde efficiency` output surface (`session`, `--worst`, `daily`,
//! `weekly`) shares -- mirroring `cost`'s single `compute_summaries` seam (design "API Design":
//! "Discovery reuses `common::scan`").

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use common::EfficiencyConfig;
use common::scan::{SessionFile, find_session_files_with_staged, pricing_files};
use eyre::{Context, Result};
use log::{debug, warn};
use rayon::prelude::*;
use sessions::EfficiencyCandidate;

use crate::extract::{self, FileEfficiency};
use crate::fold::{SessionEfficiency, fold};
use crate::outcome::{self, Outcomes};
use crate::score::scored;

/// One session's computed, scored efficiency plus the discovery-level metadata output rendering
/// needs: the last-touched LOCAL date driving `daily`/`weekly` bucketing. There is no per-record
/// timestamp retained in `RawCounters` (percentiles/ratios are the only signals that survive per
/// scope), so the session's own files' mtime is the correct-seam substitute -- the same signal
/// `common::scan::filter_by_date_range`'s date prefilter already uses.
///
/// `outcomes` is populated ONLY on the reindex path ([`collect_layouts`], which passes a `repo_root`):
/// the catalog persists per-session outcomes (Phase 2), but the live `clyde efficiency` surfaces
/// (`session`/`daily`/`weekly`/`--worst`) do not render them, so [`collect_all`]/[`collect_matching`]
/// skip the second per-file scan and leave this at its all-empty default.
#[derive(Debug, Clone)]
pub struct CollectedSession {
    pub session_id: String,
    pub last_active: DateTime<Local>,
    pub efficiency: SessionEfficiency,
    pub outcomes: Outcomes,
}

/// Discover every session under `projects_dir`, compute + score each one's [`SessionEfficiency`].
/// A file that fails to extract is warn-and-skipped (house robustness contract); the rest of its
/// group still contributes. Sessions are computed in parallel (`extract` re-reads each
/// page-cache-hot file, same shape as `report`'s collect `par_iter`).
pub fn collect_all(
    projects_dir: &Path,
    staged_root: &Path,
    config: &EfficiencyConfig,
) -> Result<Vec<CollectedSession>> {
    debug!(
        "collect_all: projects_dir={} staged_root={}",
        projects_dir.display(),
        staged_root.display()
    );
    let files = find_session_files_with_staged(projects_dir, staged_root)
        .context("collect_all: failed to scan session files")?;
    let groups = group_by_session(&files);
    debug!("collect_all: files={} sessions={}", files.len(), groups.len());

    let sessions: Vec<CollectedSession> = groups
        .par_iter()
        .map(|(session_id, group_files)| build_session(session_id, group_files, config, None))
        .collect();

    Ok(sessions)
}

/// Discover and compute only the session group(s) whose id starts with `id` (mirrors `cost`'s
/// `Command::Session` id-prefix match). Returns zero, one, or more than one match (an ambiguous
/// prefix) so the caller decides how to report each case.
pub fn collect_matching(
    projects_dir: &Path,
    staged_root: &Path,
    id: &str,
    config: &EfficiencyConfig,
) -> Result<Vec<CollectedSession>> {
    debug!(
        "collect_matching: projects_dir={} staged_root={} id={id}",
        projects_dir.display(),
        staged_root.display()
    );
    let files = find_session_files_with_staged(projects_dir, staged_root)
        .context("collect_matching: failed to scan session files")?;
    let groups = group_by_session(&files);

    let matches: Vec<CollectedSession> = groups
        .iter()
        .filter(|(session_id, _)| session_id.starts_with(id))
        .map(|(session_id, group_files)| build_session(session_id, group_files, config, None))
        .collect();
    debug!("collect_matching: id={id} matches={}", matches.len());
    Ok(matches)
}

/// What one backfill pass found: the sessions it could compute, and the ones with no bytes left.
/// A named struct rather than a tuple so neither half can be read as the other at a call site.
#[derive(Debug, Clone, Default)]
pub struct Collected {
    pub sessions: Vec<CollectedSession>,
    /// Session ids that resolved to NO readable transcript, live or staged. Nothing to price, ever,
    /// so `sessions.len() < candidates.len()` is never a silent delta.
    pub unrecoverable: Vec<String>,
}

/// Compute the listed candidates, resolving each one's bytes through [`common::scan::pricing_files`]
/// (live layout first, staged second, subagent-only accepted).
///
/// The incremental seam behind the backfill (`efficiency::reindex_efficiency`): the catalog hands
/// over its efficiency-`NULL` rows WITH their path fields, and this prices exactly those. It does no
/// tree walk at all -- it stats only the candidates' own paths -- which is why it replaced
/// `collect_ids`, whose whole-tree scan could never see a reaped session's staged copy.
///
/// A candidate resolving to an empty file list is **unrecoverable**: its live transcript is gone and
/// no staged copy exists, so its spend cannot be recovered by any later run. It is counted and
/// warned about rather than silently dropped from the total.
///
/// This is the ONLY collector that extracts outcomes: the reindex path persists per-session outcomes
/// into the catalog's `outcome_json` column (Phase 2), so it pays the second per-file scan the live
/// surfaces skip. `repo_root` is the configured clone root the edited-file paths are bucketed
/// against for `Outcomes::repos_touched` (repo attribution's rule 3).
pub fn collect_layouts(
    candidates: &[EfficiencyCandidate],
    config: &EfficiencyConfig,
    repo_root: &Path,
) -> Result<Collected> {
    debug!(
        "collect_layouts: candidates={} repo_root={}",
        candidates.len(),
        repo_root.display()
    );
    if candidates.is_empty() {
        return Ok(Collected::default());
    }

    // Resolution is a handful of stats per candidate (no tree walk), so it runs sequentially: it is
    // cheap, and it keeps `unrecoverable` in the catalog's own stable row order rather than in
    // whatever order a parallel map happens to finish.
    let mut unrecoverable: Vec<String> = Vec::new();
    let mut priceable: Vec<(&EfficiencyCandidate, Vec<SessionFile>)> = Vec::new();
    for candidate in candidates {
        let files = pricing_files(
            &candidate.session_id,
            &candidate.transcript_path,
            Path::new(&candidate.project_dir),
            candidate.staged_path.as_deref(),
        );
        if files.is_empty() {
            // Names the REASON, not just the id: the first run after this ships emits one of these
            // per historically-reaped session, and a wall of bare ids reads as a live crash rather
            // than as the historical fact it is.
            warn!(
                "{}: no transcript on disk (live reaped, no staged copy); spend for this session is unrecoverable",
                candidate.session_id
            );
            unrecoverable.push(candidate.session_id.clone());
        } else {
            priceable.push((candidate, files));
        }
    }

    let sessions: Vec<CollectedSession> = priceable
        .par_iter()
        .map(|(candidate, files)| {
            let group_files: Vec<&SessionFile> = files.iter().collect();
            build_session(&candidate.session_id, &group_files, config, Some(repo_root))
        })
        .collect();

    debug!(
        "collect_layouts: candidates={} computed={} unrecoverable={}",
        candidates.len(),
        sessions.len(),
        unrecoverable.len()
    );
    Ok(Collected {
        sessions,
        unrecoverable,
    })
}

fn group_by_session(files: &[SessionFile]) -> BTreeMap<String, Vec<&SessionFile>> {
    let mut groups: BTreeMap<String, Vec<&SessionFile>> = BTreeMap::new();
    for f in files {
        groups.entry(f.group_id.clone()).or_default().push(f);
    }
    groups
}

/// `outcomes_repo_root` is BOTH the outcome switch and rule 3's parsing root: `Some(root)` extracts
/// outcomes (bucketing edited paths under `root`), `None` skips the second per-file scan entirely.
/// One parameter rather than a `bool` plus a path, so asking for outcomes without a root is not
/// expressible.
fn build_session(
    session_id: &str,
    group_files: &[&SessionFile],
    config: &EfficiencyConfig,
    outcomes_repo_root: Option<&Path>,
) -> CollectedSession {
    let file_effs: Vec<FileEfficiency> = group_files
        .iter()
        .filter_map(|f| match extract::extract(&f.path) {
            Ok(fe) => Some(fe),
            Err(e) => {
                warn!("collect: extract failed for {}: {e} (file skipped)", f.path.display());
                None
            }
        })
        .collect();

    // Outcomes are extracted only for the reindex path (`outcomes_repo_root` is `Some`); a per-file
    // scan failure is warn-and-skipped (same robustness contract as efficiency extract) so one bad
    // file cannot fail the whole session's annotation. An empty session unions to the all-empty
    // default (a stored, non-NULL `outcome_json` distinct from "not yet reindexed").
    let outcomes = match outcomes_repo_root {
        Some(repo_root) => {
            let file_outcomes: Vec<outcome::FileOutcomes> = group_files
                .iter()
                .filter_map(|f| match outcome::extract(&f.path) {
                    Ok(fo) => Some(fo),
                    Err(e) => {
                        warn!(
                            "collect: outcome extract failed for {}: {e} (file skipped)",
                            f.path.display()
                        );
                        None
                    }
                })
                .collect();
            outcome::union(&file_outcomes, repo_root)
        }
        None => Outcomes::default(),
    };

    let last_active: SystemTime = group_files
        .iter()
        .map(|f| f.mtime)
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let last_active: DateTime<Local> = last_active.into();

    let efficiency = scored(fold(session_id, &file_effs), config);
    debug!(
        "collect::build_session: session_id={session_id} files={} last_active={last_active} \
         with_outcomes={} commits={} prs={} repos-touched={}",
        group_files.len(),
        outcomes_repo_root.is_some(),
        outcomes.commits.len(),
        outcomes.prs.len(),
        outcomes.repos_touched.len(),
    );

    CollectedSession {
        session_id: session_id.to_string(),
        last_active,
        efficiency,
        outcomes,
    }
}

#[cfg(test)]
mod tests;
