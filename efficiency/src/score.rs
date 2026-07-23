//! Threshold flagging: derive [`EfficiencyFlag`]s on a session's whole-session AGGREGATE signals
//! against the configured `efficiency:` thresholds (design Phase 4,
//! `docs/design/2026-07-22-session-efficiency-signals.md`).
//!
//! Config supplies the *what* (where each line sits); this module applies it. Scoring is a pure
//! function of the aggregate [`EfficiencySignals`] plus the [`EfficiencyConfig`] — it returns data,
//! never mutates through side effects, so it is trivially testable and the aggregation invariant is
//! untouched (the aggregate is still `finalize(union of scopes)`; scoring only reads it).
//!
//! The **eligibility gate** (`minimum-total-tokens` / `minimum-turns`) applies ONLY to the
//! cache-waste flag: a short one-shot session cannot structurally reuse cache, so a low
//! `cache-read-share` there is not waste, it is expected — flagging it would be a false positive.
//! The tool-error-rate and auto-compaction flags are NOT gated: an error-prone or context-to-the-
//! wall session is worth surfacing regardless of size.

use common::EfficiencyConfig;
use log::debug;

use crate::fold::{EfficiencyFlag, SessionEfficiency};
use crate::metrics::{CompactionTrigger, EfficiencySignals};

/// Score one session's aggregate `signals` against `config`, returning every breached flag.
///
/// Order is deterministic (cache-waste, tool-error, auto-compaction) so callers/tests see a stable
/// list. An all-healthy eligible session, and an ineligible session below the cache floor, both
/// return an empty (or cache-flag-free) list — the gate is what makes the second case quiet.
pub fn score(signals: &EfficiencySignals, config: &EfficiencyConfig) -> Vec<EfficiencyFlag> {
    let raw = &signals.raw;
    let total_tokens = raw.total_tokens();
    let eligible = total_tokens >= config.minimum_total_tokens() && raw.turns >= config.minimum_turns();
    debug!(
        "score: total-tokens={} turns={} eligible={} (min-tokens={} min-turns={}) \
         cache-read-share={:?} floor={} tool-error-rate={:?} ceiling={} auto-compaction-flag={}",
        total_tokens,
        raw.turns,
        eligible,
        config.minimum_total_tokens(),
        config.minimum_turns(),
        signals.cache_read_share,
        config.cache_read_share_floor(),
        signals.tool_error_rate,
        config.tool_error_rate_ceiling(),
        config.auto_compaction_flag(),
    );

    let mut flags = Vec::new();

    // Cache-waste: eligible sessions only, and only when a share is actually computable.
    if eligible
        && let Some(share) = signals.cache_read_share
        && share < config.cache_read_share_floor()
    {
        flags.push(EfficiencyFlag::LowCacheReadShare {
            observed: share,
            floor: config.cache_read_share_floor(),
        });
    }

    // Tool-error rate: not gated by eligibility (an error-prone short session still matters).
    if let Some(rate) = signals.tool_error_rate
        && rate > config.tool_error_rate_ceiling()
    {
        flags.push(EfficiencyFlag::HighToolErrorRate {
            observed: rate,
            ceiling: config.tool_error_rate_ceiling(),
        });
    }

    // Auto-compaction: any auto-triggered compaction, when the flag is enabled.
    if config.auto_compaction_flag() {
        let count = raw
            .compactions
            .iter()
            .filter(|c| c.trigger == CompactionTrigger::Auto)
            .count() as u64;
        if count > 0 {
            flags.push(EfficiencyFlag::AutoCompaction { count });
        }
    }

    debug!("score: flags={} -> {flags:?}", flags.len());
    flags
}

/// Populate `session.flags` by [`score`]ing its aggregate against `config`, returning the session.
///
/// The seam callers use to attach flags after [`crate::fold`] (which is config-free and always
/// leaves `flags` empty). Kept separate from `fold` so the Phase 3 fold and its aggregation-
/// invariant tests stay config-free.
pub fn scored(mut session: SessionEfficiency, config: &EfficiencyConfig) -> SessionEfficiency {
    debug!("scored: session_id={}", session.session_id);
    session.flags = score(&session.aggregate, config);
    session
}

#[cfg(test)]
mod tests;
