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
use common::repo::SharedResolver;
use common::scan::{SessionFile, SessionFileKind, find_session_files_with_staged, pricing_files};
use eyre::{Context, Result};
use log::{debug, warn};
use rayon::prelude::*;
use sessions::EfficiencyCandidate;

use crate::extract::{self, FileEfficiency, GroupSeen};
use crate::fold::{SessionEfficiency, fold};
use crate::outcome::{self, Outcomes};
use crate::score::scored;

/// One session's computed, scored efficiency plus the discovery-level metadata output rendering
/// needs: the last-touched LOCAL date driving `daily`/`weekly` bucketing. There is no per-record
/// timestamp retained in `RawCounters` (percentiles/ratios are the only signals that survive per
/// scope), so the session's own files' mtime is the correct-seam substitute -- the same signal
/// `common::scan::filter_by_date_range`'s date prefilter already uses.
///
/// `outcomes` is populated ONLY on the reindex path ([`collect_layouts`], which passes a resolver):
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
        .map(|(session_id, group_files)| build_session(session_id, group_files, config, None).session)
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
        .map(|(session_id, group_files)| build_session(session_id, group_files, config, None).session)
        .collect();
    debug!("collect_matching: id={id} matches={}", matches.len());
    Ok(matches)
}

/// What one backfill pass found: the sessions it could compute, and the ones with no bytes left.
/// A named struct rather than a tuple so neither half can be read as the other at a call site.
#[derive(Debug, Clone, Default)]
pub struct Collected {
    pub sessions: Vec<CollectedSession>,
    /// Session ids that resolved to NO readable transcript, live or staged, and have no stored blob
    /// to fall back on. Nothing to price, ever, so `sessions.len() < candidates.len()` is never a
    /// silent delta.
    pub unrecoverable: Vec<String>,
    /// Candidates that already carry a blob (one without per-day buckets) and could not be
    /// recomputed in full: no bytes left, a file failed to extract, or the files that remain hold
    /// fewer tokens than the blob already accounts for. Their stored blob stays,
    /// because a partial or empty recompute would overwrite real accounting with less of it. The
    /// next reindex offers them again.
    pub kept: Vec<String>,
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
/// surfaces skip. `work_remote_hosts` is the allowlist the shared resolver applies when it asks git
/// where each edited file lives, which is how `Outcomes::repos_touched` (repo attribution's rule 3)
/// is bucketed. No clone root reaches here: rule 3 stopped being a path parse in v0.23.0.
pub fn collect_layouts(
    candidates: &[EfficiencyCandidate],
    config: &EfficiencyConfig,
    work_remote_hosts: &[String],
) -> Result<Collected> {
    debug!(
        "collect_layouts: candidates={} work_remote_hosts={}",
        candidates.len(),
        work_remote_hosts.len()
    );
    if candidates.is_empty() {
        return Ok(Collected::default());
    }

    // Resolution is a handful of stats per candidate (no tree walk), so it runs sequentially: it is
    // cheap, and it keeps `unrecoverable` in the catalog's own stable row order rather than in
    // whatever order a parallel map happens to finish.
    let mut unrecoverable: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut priceable: Vec<(&EfficiencyCandidate, Vec<SessionFile>)> = Vec::new();
    for candidate in candidates {
        let files = pricing_files(
            &candidate.session_id,
            &candidate.transcript_path,
            Path::new(&candidate.project_dir),
            candidate.staged_path.as_deref(),
        );
        if files.is_empty() && candidate.has_blob() {
            // A bucket-less blob whose bytes are gone: the stored accounting is all there will ever
            // be, so it stays. Not unrecoverable -- its spend is already in the catalog.
            debug!(
                "collect_layouts: session_id={} has no bytes left to bucket; keeping its stored blob",
                candidate.session_id
            );
            kept.push(candidate.session_id.clone());
        } else if files.is_empty() {
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

    // ONE resolver for the whole pass, shared by reference across the rayon threads. That sharing is
    // the point: sessions overlap heavily on directories, so a per-session memo would re-probe the
    // same checkout once per session instead of once per catalog.
    // `with_hosts`, NOT `new`: rule 3 must be able to refuse a work-looking slug that arrived from
    // a non-allowlisted remote, which is the same gate rule 1 applies.
    let slugs = SharedResolver::with_hosts(work_remote_hosts);
    let built: Vec<(&EfficiencyCandidate, BuiltSession)> = priceable
        .par_iter()
        .map(|(candidate, files)| {
            let group_files: Vec<&SessionFile> = files.iter().collect();
            (
                *candidate,
                build_session(&candidate.session_id, &group_files, config, Some(&slugs)),
            )
        })
        .collect();

    // A recompute that lost a file keeps the old blob: overwriting it would trade stored accounting
    // for a partial one. So does a recompute that read every file and still saw FEWER tokens than
    // the stored blob accounts for (a staged copy taken before the session ended): the bytes on disk
    // are a truncated view of what the blob already priced. A row with no blob has nothing to keep,
    // so a partial recompute still beats none for it, as before.
    //
    // "Fewer" is measured on the BLOB'S OWN basis. A blob without `skill-calls` was priced per file
    // (copies counted again, first record per id), so it is compared against the recompute's
    // per-file figure: the group-wide dedupe always sees fewer tokens and must not trip the guard,
    // while a truncated staged copy still does. A blob with `skill-calls` was priced group-wide, so
    // it is compared against the deduped total. The guard never compares across rules.
    let mut sessions: Vec<CollectedSession> = Vec::with_capacity(built.len());
    for (candidate, built) in built {
        let BuiltSession {
            session,
            failed,
            tokens_file_local,
        } = built;
        let (recomputed, basis) = if candidate.stored_has_skill_calls {
            (session.efficiency.aggregate.raw.total_tokens(), "group-wide")
        } else {
            (tokens_file_local, "per-file")
        };
        if failed > 0 && candidate.has_blob() {
            warn!(
                "{}: {failed} transcript file(s) failed to extract; keeping the stored efficiency blob rather \
                 than overwriting it with a partial recompute",
                candidate.session_id
            );
            kept.push(candidate.session_id.clone());
        } else if let Some(stored) = candidate.stored_tokens
            && recomputed < stored
        {
            warn!(
                "{}: recompute saw {recomputed} tokens ({basis} basis, the rule the stored blob was priced under) \
                 but the stored efficiency blob accounts for {stored}; keeping the stored blob rather than \
                 overwriting it with less accounting",
                candidate.session_id
            );
            kept.push(candidate.session_id.clone());
        } else {
            sessions.push(session);
        }
    }

    debug!(
        "collect_layouts: candidates={} computed={} unrecoverable={} kept={}",
        candidates.len(),
        sessions.len(),
        unrecoverable.len(),
        kept.len()
    );
    Ok(Collected {
        sessions,
        unrecoverable,
        kept,
    })
}

fn group_by_session(files: &[SessionFile]) -> BTreeMap<String, Vec<&SessionFile>> {
    let mut groups: BTreeMap<String, Vec<&SessionFile>> = BTreeMap::new();
    for f in files {
        groups.entry(f.group_id.clone()).or_default().push(f);
    }
    groups
}

/// Where a file sorts in its session group's read order. The ORIGINAL of a copied record must be
/// read before its copies, because the first holder's scope keeps a shared message (the totals do
/// not depend on order; see `extract::GroupSeen`): the parent, then the subagents that copy
/// nothing, then the forks, which copy from one of those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FileClass {
    Parent,
    Subagent,
    Fork,
}

/// Is this subagent transcript a fork? Two independent markers, either one suffices:
/// - its sibling `<agent>.meta.json` has `"isFork": true` (the only marker a fork of a SUBAGENT
///   carries, since that shape copies its source's history inline);
/// - its first record is `{"type":"fork-context-ref",...}` (a fork of the MAIN session references
///   the parent's context instead of copying it). This one survives staging, which before Phase 2
///   copied no meta files.
///
/// An unreadable file or meta is "not a fork": it sorts with the plain subagents, which can shift
/// only the per-agent split, never the totals.
fn is_fork(path: &Path) -> bool {
    let meta = path.with_extension("meta.json");
    match std::fs::read_to_string(&meta) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) if v.get("isFork").and_then(serde_json::Value::as_bool) == Some(true) => {
                debug!("collect::is_fork: {} is a fork (meta isFork)", path.display());
                return true;
            }
            Ok(_) => {}
            Err(e) => warn!(
                "collect::is_fork: unparseable meta {}: {e} (falling back to the fork-context-ref check)",
                meta.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(
            "collect::is_fork: failed to read meta {}: {e} (falling back to the fork-context-ref check)",
            meta.display()
        ),
    }
    let fork = first_record_is_fork_ref(path);
    debug!("collect::is_fork: {} fork-context-ref={fork}", path.display());
    fork
}

/// Whether the file's first non-empty line parses as a record of `type == "fork-context-ref"`.
fn first_record_is_fork_ref(path: &Path) -> bool {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let first = std::io::BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .find(|l| !l.trim().is_empty());
    first
        .and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
        .is_some_and(|v| v.get("type").and_then(serde_json::Value::as_str) == Some("fork-context-ref"))
}

fn file_class(file: &SessionFile) -> FileClass {
    match file.kind {
        SessionFileKind::Parent => FileClass::Parent,
        SessionFileKind::Subagent if is_fork(&file.path) => FileClass::Fork,
        SessionFileKind::Subagent => FileClass::Subagent,
    }
}

/// The group's read order: parent, then non-fork subagents, then forks, ties by path (the order the
/// scan already yields). A fork of a fork therefore sorts by path against its source fork, not by
/// `spawnDepth`; the totals are unaffected, only which fork's scope keeps a shared turn.
fn order_group<'a>(group_files: &[&'a SessionFile]) -> Vec<&'a SessionFile> {
    let mut keyed: Vec<(FileClass, &SessionFile)> = group_files.iter().map(|f| (file_class(f), *f)).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    debug!(
        "collect::order_group: files={} forks={}",
        keyed.len(),
        keyed.iter().filter(|(c, _)| *c == FileClass::Fork).count()
    );
    keyed.into_iter().map(|(_, f)| f).collect()
}

/// A session group's efficiency extraction: the per-file results (plus the group's applied usage as
/// one more), how many files failed, and the old-rule per-file token sum the keep-blob guard reads.
struct GroupExtract {
    files: Vec<FileEfficiency>,
    failed: usize,
    tokens_file_local: u64,
}

/// Extract `ordered` files through ONE [`GroupSeen`], in the given order, then apply the group's
/// buffered usage once. A file that fails to extract is warn-and-skipped (the rest still count).
fn extract_ordered(ordered: &[&SessionFile]) -> GroupExtract {
    let mut group = GroupSeen::default();
    let mut files: Vec<FileEfficiency> = Vec::with_capacity(ordered.len() + 1);
    for f in ordered {
        match extract::extract(&f.path, &mut group) {
            Ok(fe) => files.push(fe),
            Err(e) => warn!("collect: extract failed for {}: {e} (file skipped)", f.path.display()),
        }
    }
    let failed = ordered.len() - files.len();
    let tokens_file_local = files.iter().map(|f| f.tokens_file_local).sum();
    files.push(group.finish());
    GroupExtract {
        files,
        failed,
        tokens_file_local,
    }
}

/// What [`build_session`] hands back: the session, how many of its files failed to extract (warned
/// and skipped), and the group's token sum under the OLD per-file rule, so the reindex path can
/// refuse to overwrite a stored blob with a partial or truncated recompute on the blob's own basis.
struct BuiltSession {
    session: CollectedSession,
    failed: usize,
    tokens_file_local: u64,
}

/// `slugs` is BOTH the outcome switch and rule 3's resolver: `Some(resolver)` extracts outcomes
/// (bucketing edited paths by asking git about each parent directory), `None` skips the second
/// per-file scan entirely. One parameter rather than a `bool` plus a resolver, so asking for outcomes
/// without one is not expressible.
///
/// It replaces the clone root this used to take. Rule 3 no longer reads the `<root>/<org>/<repo>`
/// shape at all (Problem 5), so a root is no longer the thing it needs; rule 4 still reads the
/// configured roots, but rule 4 lives in `common::repo` and never came through here.
///
/// Both passes read the group in [`order_group`]'s order with group-wide seen-state, so a record a
/// fork copied from its source counts once.
fn build_session(
    session_id: &str,
    group_files: &[&SessionFile],
    config: &EfficiencyConfig,
    slugs: Option<&SharedResolver>,
) -> BuiltSession {
    let ordered = order_group(group_files);
    let GroupExtract {
        files: file_effs,
        failed,
        tokens_file_local,
    } = extract_ordered(&ordered);

    // Outcomes are extracted only for the reindex path (`slugs` is `Some`); a per-file
    // scan failure is warn-and-skipped (same robustness contract as efficiency extract) so one bad
    // file cannot fail the whole session's annotation. An empty session unions to the all-empty
    // default (a stored, non-NULL `outcome_json` distinct from "not yet reindexed").
    let outcomes = match slugs {
        Some(slugs) => {
            let mut seen = outcome::OutcomeSeen::default();
            let file_outcomes: Vec<outcome::FileOutcomes> = ordered
                .iter()
                .filter_map(|f| match outcome::extract(&f.path, &mut seen) {
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
            outcome::union(&file_outcomes, slugs)
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
        "collect::build_session: session_id={session_id} files={} failed={failed} last_active={last_active} \
         with_outcomes={} commits={} prs={} repos-touched={} tokens={} tokens-file-local={tokens_file_local}",
        group_files.len(),
        slugs.is_some(),
        outcomes.commits.len(),
        outcomes.prs.len(),
        outcomes.repos_touched.len(),
        efficiency.aggregate.raw.total_tokens(),
    );

    BuiltSession {
        session: CollectedSession {
            session_id: session_id.to_string(),
            last_active,
            efficiency,
            outcomes,
        },
        failed,
        tokens_file_local,
    }
}

#[cfg(test)]
mod tests;
