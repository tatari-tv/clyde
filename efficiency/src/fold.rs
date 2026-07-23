//! Fold per-file [`FileEfficiency`] results for one session group into a [`SessionEfficiency`]: the
//! per-subagent breakdown PLUS the whole-session aggregate.
//!
//! This is where the design's **Aggregation invariant** (design doc lines ~147) is enforced in
//! code. The decomposition is canonical: each subagent's [`RawCounters`] are unioned across the
//! group's files, and the aggregate is `finalize(parent_own ⊎ every subagent's counters)` -- a
//! ratio-of-sums for the cache ratios and a percentile recompute over the UNIONED duration sample,
//! NEVER a field-sum or average of the sub-scope derived metrics. Nothing in [`SessionEfficiency`]
//! is a stored redundant number that could diverge from the counters it was computed from; the
//! `aggregate_equals_recompute_of_parent_and_subagents` test pins exactly that.

use std::collections::BTreeMap;

use log::debug;

use crate::extract::{FileEfficiency, SubagentRaw};
use crate::metrics::{EfficiencySignals, RawCounters, finalize};

/// Signals for one subagent scope, tagged with its `agentId` and (if known) its `attributionAgent`
/// TYPE. The `signals` are `finalize`d from the subagent's OWN unioned counters.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentEfficiency {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub signals: EfficiencySignals,
}

/// A flagged efficiency breach. Scored against configurable thresholds in Phase 4; Phase 3 always
/// yields an empty `flags` list (this enum has no variants yet), so the field exists in the Data
/// Model shape without pretending to score anything.
#[derive(Debug, Clone, PartialEq)]
pub enum EfficiencyFlag {}

/// One session's full efficiency picture: the recomputed whole-session `aggregate`, the canonical
/// per-subagent `subagents` breakdown, and (Phase 4) any scored `flags`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEfficiency {
    pub session_id: String,
    pub aggregate: EfficiencySignals,
    pub subagents: Vec<SubagentEfficiency>,
    pub flags: Vec<EfficiencyFlag>,
}

/// Fold every per-file result for ONE session group into its [`SessionEfficiency`].
///
/// Steps, in order: (1) union parent-scope counters across the group's files into `parent_own`;
/// (2) union subagent counters by `agentId` across files; (3) recompute the aggregate from the
/// union of `parent_own` and every subagent's counters (the invariant); (4) `finalize` each
/// subagent for the breakdown. `session_id` is the caller-supplied group id (the scan
/// `group_id` -- a parent file and its subagents share it), authoritative over any per-file
/// `sessionId`.
pub fn fold(session_id: &str, files: &[FileEfficiency]) -> SessionEfficiency {
    debug!("fold: session_id={session_id} files={}", files.len());

    let mut parent_own = RawCounters::default();
    let mut subagents: BTreeMap<String, SubagentRaw> = BTreeMap::new();

    for file in files {
        parent_own.merge(&file.parent);
        for (agent_id, sub) in &file.subagents {
            subagents.entry(agent_id.clone()).or_default().merge(sub);
        }
    }

    // The invariant: aggregate = finalize(parent_own ⊎ all subagents' counters). Recomputed from
    // the unioned RAW counters, never field-summed from the sub-scope derived metrics.
    let mut aggregate_raw = parent_own.clone();
    for sub in subagents.values() {
        aggregate_raw.merge(&sub.raw);
    }
    let aggregate = finalize(aggregate_raw);

    let subagent_breakdown: Vec<SubagentEfficiency> = subagents
        .into_iter()
        .map(|(agent_id, sub)| SubagentEfficiency {
            agent_id,
            agent_type: sub.agent_type,
            signals: finalize(sub.raw),
        })
        .collect();

    debug!(
        "fold: session_id={session_id} subagents={} aggregate-turns={} aggregate-tool-errors={} \
         aggregate-cost-usd={}",
        subagent_breakdown.len(),
        aggregate.raw.turns,
        aggregate.raw.tool_errors,
        aggregate.raw.cost_usd,
    );

    SessionEfficiency {
        session_id: session_id.to_string(),
        aggregate,
        subagents: subagent_breakdown,
        flags: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
