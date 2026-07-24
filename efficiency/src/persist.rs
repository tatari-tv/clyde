//! Phase 6 backfill: compute efficiency for un-annotated catalog sessions and persist it, WITHOUT
//! advancing the export cursor.
//!
//! This closes the gap the review panel caught: `sessions::Db::upsert_session` skips a row whose
//! transcript mtime is unchanged, so a bare v6 migration would leave every EXISTING session's
//! efficiency `NULL` forever. [`reindex_efficiency`] instead drives off the DB's own
//! `efficiency IS NULL` predicate ([`sessions::Db::sessions_missing_efficiency`]) — independent of
//! the mtime skip-key — recomputes exactly those sessions from disk, and writes them through
//! [`sessions::Db::set_efficiency_many`] (which suppresses the revision trigger so writing a derived
//! annotation never bumps `updated_at`).
//!
//! The three flat ranking scalars (`cache_read_share`, `tool_errors`, `cost_usd`) are pulled from
//! the SAME computed [`SessionEfficiency`] that is serialized into `efficiency_json`, so an indexed
//! scalar can never diverge from the JSON it was materialized from (single computation path).

use std::collections::BTreeSet;
use std::path::Path;

use common::EfficiencyConfig;
use eyre::{Context, Result};
use log::{debug, info};
use serde::Serialize;
use sessions::{Db, EfficiencyWrite};

use crate::collect::{CollectedSession, collect_ids};

/// Outcome of one [`reindex_efficiency`] pass. `Serialize` (kebab-case) so the clyde binary can emit
/// it as JSON on a piped `session reindex`, mirroring `ReindexStats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PersistStats {
    /// Rows the catalog reported as un-annotated (`efficiency_json IS NULL`, non-archived).
    pub candidates: usize,
    /// Of those, the sessions actually found on disk and computed (a candidate whose transcript has
    /// vanished but is not yet reconciled contributes a candidate with no computed result).
    pub computed: usize,
    /// Rows actually updated by the write (equals `computed` in the normal case; a computed session
    /// whose id is no longer in the catalog would update 0 rows).
    pub written: usize,
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
    /// pull the three ranking scalars from the SAME aggregate — the single computation path that keeps
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
pub fn reindex_efficiency(db: &Db, projects_dir: &Path, config: &EfficiencyConfig) -> Result<PersistStats> {
    debug!("reindex_efficiency: projects_dir={}", projects_dir.display());
    let missing: BTreeSet<String> = db
        .sessions_missing_efficiency()
        .context("reindex_efficiency: failed to query sessions missing efficiency")?
        .into_iter()
        .collect();
    debug!("reindex_efficiency: candidates={}", missing.len());

    let sessions: Vec<CollectedSession> = collect_ids(projects_dir, &missing, config)?;
    let owned: Vec<OwnedEfficiency> = sessions
        .iter()
        .map(OwnedEfficiency::from_session)
        .collect::<Result<_>>()?;
    let writes: Vec<EfficiencyWrite<'_>> = owned.iter().map(OwnedEfficiency::as_write).collect();
    let written = db
        .set_efficiency_many(&writes)
        .context("reindex_efficiency: failed to persist efficiency annotations")?;

    let stats = PersistStats {
        candidates: missing.len(),
        computed: sessions.len(),
        written,
    };
    info!(
        "reindex_efficiency: candidates={} computed={} written={} (updated_at unchanged)",
        stats.candidates, stats.computed, stats.written
    );
    Ok(stats)
}

#[cfg(test)]
mod tests;
