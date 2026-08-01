//! Phase 6 backfill: compute efficiency for un-annotated catalog sessions and persist it, WITHOUT
//! advancing the export cursor.
//!
//! This closes the gap the review panel caught: `sessions::Db::upsert_session` skips a row whose
//! transcript mtime is unchanged, so a bare v6 migration would leave every EXISTING session's
//! efficiency `NULL` forever. [`reindex_efficiency`] instead drives off the DB's own
//! `efficiency IS NULL` predicate ([`sessions::Db::sessions_missing_efficiency`]) -- independent of
//! the mtime skip-key -- recomputes exactly those sessions from disk, and writes them through
//! [`sessions::Db::set_efficiency_many`] (which suppresses the revision trigger so writing a derived
//! annotation never bumps `updated_at`).
//!
//! The three flat ranking scalars (`cache_read_share`, `tool_errors`, `cost_usd`) are pulled from
//! the SAME computed [`SessionEfficiency`] that is serialized into `efficiency_json`, so an indexed
//! scalar can never diverge from the JSON it was materialized from (single computation path).

use std::path::Path;

use common::EfficiencyConfig;
use eyre::{Context, Result};
use log::{debug, info};
use serde::Serialize;
use sessions::{Db, EfficiencyWrite};

use crate::collect::{CollectedSession, collect_layouts};

/// Outcome of one [`reindex_efficiency`] pass. `Serialize` (kebab-case) so the clyde binary can emit
/// it as JSON on a piped `session reindex`, mirroring `ReindexStats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PersistStats {
    /// Rows the catalog reported as un-annotated (`efficiency_json IS NULL`), archived or not.
    pub candidates: usize,
    /// Of those, the sessions whose bytes resolved live-or-staged and were computed.
    pub computed: usize,
    /// Rows actually updated by the write (equals `computed` in the normal case; a computed session
    /// whose id is no longer in the catalog would update 0 rows).
    pub written: usize,
    /// Candidates with NO readable transcript, live or staged: nothing left to price, ever.
    /// Reported so `computed < candidates` is never a silent delta.
    pub unrecoverable: usize,
    /// Of the computed sessions, those whose aggregate names at least one model the embedded feed
    /// could not price (`RawCounters::unpriced_models` non-empty). Every one of those sessions has a
    /// `cost_usd` that is LOW by an unknown amount, so this is the count of catalog rows whose
    /// dollars cannot be trusted. Zero-token unpriced models are excluded upstream, so a non-zero
    /// count here always means real tokens went unpriced.
    pub unpriced: usize,
}

/// One computed session's efficiency + outcomes in owned form, so the borrowing [`EfficiencyWrite`]s
/// handed to [`Db::set_efficiency_many`] can reference stable storage across the whole batch.
struct OwnedEfficiency {
    session_id: String,
    efficiency_json: String,
    cache_read_share: Option<f64>,
    tool_errors: i64,
    cost_usd: f64,
    outcome_json: String,
}

impl OwnedEfficiency {
    /// Serialize the whole nested [`SessionEfficiency`] AND the per-session [`Outcomes`] to JSON, and
    /// pull the three ranking scalars from the SAME aggregate -- the single computation path that keeps
    /// the indexed scalars and the efficiency JSON in lock step. `outcome_json` is always a concrete
    /// object (the all-empty default for a session with no observed outcome), never NULL, so a
    /// reindexed row is distinguishable from a not-yet-reindexed one.
    fn from_session(cs: &CollectedSession) -> Result<Self> {
        let aggregate = &cs.efficiency.aggregate;
        let efficiency_json = serde_json::to_string(&cs.efficiency)
            .with_context(|| format!("reindex_efficiency: serialize efficiency for session {}", cs.session_id))?;
        let outcome_json = serde_json::to_string(&cs.outcomes)
            .with_context(|| format!("reindex_efficiency: serialize outcomes for session {}", cs.session_id))?;
        Ok(Self {
            session_id: cs.session_id.clone(),
            efficiency_json,
            cache_read_share: aggregate.cache_read_share,
            tool_errors: aggregate.raw.tool_errors as i64,
            cost_usd: aggregate.raw.cost_usd,
            outcome_json,
        })
    }

    fn as_write(&self) -> EfficiencyWrite<'_> {
        EfficiencyWrite {
            session_id: &self.session_id,
            efficiency_json: &self.efficiency_json,
            cache_read_share: self.cache_read_share,
            tool_errors: self.tool_errors,
            cost_usd: self.cost_usd,
            outcome_json: &self.outcome_json,
        }
    }
}

/// Compute and persist efficiency for every catalog session that has none yet.
///
/// Idempotent by construction: it only touches rows where `efficiency_json IS NULL`, and the write
/// does not advance `updated_at`, so running it repeatedly annotates newly-indexed (and grown, since
/// `upsert_session` NULLs efficiency on a content change) sessions without ever re-touching or
/// re-bumping an already-annotated one.
///
/// `repo_root` reaches `outcome::union`, which buckets each session's edited-file paths into
/// `Outcomes::repos_touched` (repo attribution's rule 3). That coupling is why the single
/// `efficiency_json IS NULL` predicate is enough for steady state: a grown transcript NULLs
/// efficiency, this pass re-picks the row, and `repos_touched` is recomputed with it.
///
/// Takes no `projects_dir`: each candidate row carries its own `transcript_path` / `project_dir` /
/// `staged_path`, and `collect_layouts` resolves the bytes per row (live layout first, staged
/// second). That is what lets an ARCHIVED row be priced from its staged copy, which the previous
/// whole-tree scan structurally could not do. Candidates with no bytes anywhere are counted in
/// [`PersistStats::unrecoverable`] rather than silently vanishing from the total.
pub fn reindex_efficiency(
    db: &Db,
    config: &EfficiencyConfig,
    repo_root: &Path,
    work_remote_hosts: &[String],
) -> Result<PersistStats> {
    debug!("reindex_efficiency: repo_root={}", repo_root.display());
    let candidates = db
        .sessions_missing_efficiency()
        .context("reindex_efficiency: failed to query sessions missing efficiency")?;
    debug!("reindex_efficiency: candidates={}", candidates.len());

    let collected = collect_layouts(&candidates, config, repo_root, work_remote_hosts)?;
    let owned: Vec<OwnedEfficiency> = collected
        .sessions
        .iter()
        .map(OwnedEfficiency::from_session)
        .collect::<Result<_>>()?;
    let writes: Vec<EfficiencyWrite<'_>> = owned.iter().map(OwnedEfficiency::as_write).collect();
    let written = db
        .set_efficiency_many(&writes)
        .context("reindex_efficiency: failed to persist efficiency annotations")?;

    let stats = PersistStats {
        candidates: candidates.len(),
        computed: collected.sessions.len(),
        written,
        unrecoverable: collected.unrecoverable.len(),
        unpriced: collected
            .sessions
            .iter()
            .filter(|cs| !cs.efficiency.aggregate.raw.unpriced_models.is_empty())
            .count(),
    };
    info!(
        "reindex_efficiency: candidates={} computed={} written={} unrecoverable={} unpriced={} (updated_at unchanged)",
        stats.candidates, stats.computed, stats.written, stats.unrecoverable, stats.unpriced
    );
    Ok(stats)
}

#[cfg(test)]
mod tests;
