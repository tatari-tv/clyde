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
use serde::{Deserialize, Serialize};

use crate::extract::{FileEfficiency, SubagentRaw, name_from_agent_id};
use crate::metrics::{EfficiencySignals, RawCounters, finalize};

/// Signals for one subagent scope, tagged with its `agentId` and (if known) its `attributionAgent`
/// TYPE. The `signals` are `finalize`d from the subagent's OWN unioned counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SubagentEfficiency {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub signals: EfficiencySignals,
}

/// A flagged efficiency breach, scored on the whole-session aggregate against the configured
/// `efficiency:` thresholds ([`crate::score`]). Each variant names the breached signal AND carries
/// the observed value alongside the threshold it crossed, so a flag is self-describing and legible
/// (fail loudly, per the house rule) — a consumer never has to re-derive why the session tripped.
///
/// The persisted serde shape is the internally-tagged `{ "kind": "...", ... }` form -- byte-identical
/// to the CLI's [`crate::output::FlagJson`] rendering -- so the flag reads the same whether it comes
/// from the `efficiency_json` catalog column or a live `clyde efficiency` render (siblings behave
/// identically).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EfficiencyFlag {
    /// `cache-read-share` fell below `cache-read-share-floor` on an ELIGIBLE session (one past both
    /// the `minimum-total-tokens` and `minimum-turns` gates). Cache-waste.
    LowCacheReadShare { observed: f64, floor: f64 },
    /// `tool-error rate` (`tool_errors / tool_calls`) rose above `tool-error-rate-ceiling`.
    HighToolErrorRate { observed: f64, ceiling: f64 },
    /// The session auto-compacted at least once (`auto-compaction-flag` is on). `count` is how many
    /// auto-compactions occurred.
    AutoCompaction { count: u64 },
}

/// One session's full efficiency picture: the recomputed whole-session `aggregate`, the canonical
/// per-subagent `subagents` breakdown, and (Phase 4) any scored `flags`.
///
/// Phase 6 persists this whole nested value as the catalog's `efficiency_json` TEXT column (kebab-case
/// JSON); the three ranking scalars (`aggregate.cache-read-share`, `aggregate.raw.tool-errors`,
/// `aggregate.raw.cost-usd`) ALSO land in flat indexed columns, materialized from THIS struct in one
/// computation so an indexed scalar can never diverge from the JSON it was taken from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
    // Union the name->type spawn map across the group (the parent file spawns most named agents; a
    // subagent that itself spawns contributes too). First value for a name wins.
    let mut spawn_types: BTreeMap<String, String> = BTreeMap::new();

    for file in files {
        parent_own.merge(&file.parent);
        for (agent_id, sub) in &file.subagents {
            subagents.entry(agent_id.clone()).or_default().merge(sub);
        }
        for (name, stype) in &file.spawn_types {
            spawn_types.entry(name.clone()).or_insert_with(|| stype.clone());
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
        .map(|(agent_id, sub)| {
            let SubagentRaw { agent_type, raw } = sub;
            let agent_type = resolve_agent_type(&agent_id, agent_type.as_deref(), &spawn_types);
            SubagentEfficiency {
                agent_id,
                agent_type,
                signals: finalize(raw),
            }
        })
        .collect();

    let unknown = subagent_breakdown.iter().filter(|s| s.agent_type.is_none()).count();
    debug!(
        "fold: session_id={session_id} subagents={} spawn-types={} unknown-agent-types={} \
         aggregate-turns={} aggregate-tool-errors={} aggregate-cost-usd={}",
        subagent_breakdown.len(),
        spawn_types.len(),
        unknown,
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

/// Resolve a subagent's TYPE with a four-tier fallback chain:
///
/// 1. an `attributionAgent` observed on the subagent's own records — authoritative, and how a
///    classic inline subagent (e.g. `phase-implementer`) is already labeled;
/// 2. else the `subagent_type` of the matching NAMED spawn, keyed by the name embedded in the
///    `agentId` (`a<name>-<hash>`) — recovers herdr / workflow / `Agent`-with-a-`name` subagents
///    whose sidecar records never carry `attributionAgent`;
/// 3. else the bare spawn name as a name-only label, when the spawn tool_use is not in the group
///    (e.g. a teammate launched outside the `Agent` tool, so no `subagent_type` was ever recorded);
/// 4. else `None` — a hash-only `agentId` (`a<hex>`) with no recoverable name stays `unknown`.
fn resolve_agent_type(
    agent_id: &str,
    attribution: Option<&str>,
    spawn_types: &BTreeMap<String, String>,
) -> Option<String> {
    if let Some(a) = attribution {
        return Some(a.to_string());
    }
    let name = name_from_agent_id(agent_id)?;
    Some(spawn_types.get(name).cloned().unwrap_or_else(|| name.to_string()))
}

#[cfg(test)]
mod tests;
