//! Discovery + per-session efficiency computation: scans the Claude projects tree via
//! `common::scan`, groups files by session (`group_id`), extracts + folds + scores each group.
//! This is the seam every `clyde efficiency` output surface (`session`, `--worst`, `daily`,
//! `weekly`) shares -- mirroring `cost`'s single `compute_summaries` seam (design "API Design":
//! "Discovery reuses `common::scan`").

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use common::EfficiencyConfig;
use common::scan::{SessionFile, find_session_files};
use eyre::{Context, Result};
use log::{debug, warn};
use rayon::prelude::*;

use crate::extract::{self, FileEfficiency};
use crate::fold::{SessionEfficiency, fold};
use crate::score::scored;

/// One session's computed, scored efficiency plus the discovery-level metadata output rendering
/// needs: the last-touched LOCAL date driving `daily`/`weekly` bucketing. There is no per-record
/// timestamp retained in `RawCounters` (percentiles/ratios are the only signals that survive per
/// scope), so the session's own files' mtime is the correct-seam substitute -- the same signal
/// `common::scan::filter_by_date_range`'s date prefilter already uses.
#[derive(Debug, Clone)]
pub struct CollectedSession {
    pub session_id: String,
    pub last_active: DateTime<Local>,
    pub efficiency: SessionEfficiency,
}

/// Discover every session under `projects_dir`, compute + score each one's [`SessionEfficiency`].
/// A file that fails to extract is warn-and-skipped (house robustness contract); the rest of its
/// group still contributes. Sessions are computed in parallel (`extract` re-reads each
/// page-cache-hot file, same shape as `report`'s collect `par_iter`).
pub fn collect_all(projects_dir: &Path, config: &EfficiencyConfig) -> Result<Vec<CollectedSession>> {
    debug!("collect_all: projects_dir={}", projects_dir.display());
    let files = find_session_files(projects_dir).context("collect_all: failed to scan session files")?;
    let groups = group_by_session(&files);
    debug!("collect_all: files={} sessions={}", files.len(), groups.len());

    let sessions: Vec<CollectedSession> = groups
        .par_iter()
        .map(|(session_id, group_files)| build_session(session_id, group_files, config))
        .collect();

    Ok(sessions)
}

/// Discover and compute only the session group(s) whose id starts with `id` (mirrors `cost`'s
/// `Command::Session` id-prefix match). Returns zero, one, or more than one match (an ambiguous
/// prefix) so the caller decides how to report each case.
pub fn collect_matching(projects_dir: &Path, id: &str, config: &EfficiencyConfig) -> Result<Vec<CollectedSession>> {
    debug!("collect_matching: projects_dir={} id={id}", projects_dir.display());
    let files = find_session_files(projects_dir).context("collect_matching: failed to scan session files")?;
    let groups = group_by_session(&files);

    let matches: Vec<CollectedSession> = groups
        .iter()
        .filter(|(session_id, _)| session_id.starts_with(id))
        .map(|(session_id, group_files)| build_session(session_id, group_files, config))
        .collect();
    debug!("collect_matching: id={id} matches={}", matches.len());
    Ok(matches)
}

/// Discover and compute ONLY the session groups whose id is in `ids`. The incremental seam behind
/// the Phase 6 backfill (`efficiency::reindex_efficiency`): the catalog hands the set of
/// efficiency-`NULL` session ids, and this recomputes exactly those (skipping the many already-
/// annotated sessions) rather than the whole tree. Empty `ids` is an empty result (no scan work
/// beyond the directory walk). Parallel over the matched groups, same shape as [`collect_all`].
pub fn collect_ids(
    projects_dir: &Path,
    ids: &BTreeSet<String>,
    config: &EfficiencyConfig,
) -> Result<Vec<CollectedSession>> {
    debug!("collect_ids: projects_dir={} ids={}", projects_dir.display(), ids.len());
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let files = find_session_files(projects_dir).context("collect_ids: failed to scan session files")?;
    let groups = group_by_session(&files);
    let sessions: Vec<CollectedSession> = groups
        .par_iter()
        .filter(|(session_id, _)| ids.contains(*session_id))
        .map(|(session_id, group_files)| build_session(session_id, group_files, config))
        .collect();
    debug!("collect_ids: requested={} computed={}", ids.len(), sessions.len());
    Ok(sessions)
}

fn group_by_session(files: &[SessionFile]) -> BTreeMap<String, Vec<&SessionFile>> {
    let mut groups: BTreeMap<String, Vec<&SessionFile>> = BTreeMap::new();
    for f in files {
        groups.entry(f.group_id.clone()).or_default().push(f);
    }
    groups
}

fn build_session(session_id: &str, group_files: &[&SessionFile], config: &EfficiencyConfig) -> CollectedSession {
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

    let last_active: SystemTime = group_files
        .iter()
        .map(|f| f.mtime)
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let last_active: DateTime<Local> = last_active.into();

    let efficiency = scored(fold(session_id, &file_effs), config);
    debug!(
        "collect::build_session: session_id={session_id} files={} last_active={last_active}",
        group_files.len()
    );

    CollectedSession {
        session_id: session_id.to_string(),
        last_active,
        efficiency,
    }
}

#[cfg(test)]
mod tests;
