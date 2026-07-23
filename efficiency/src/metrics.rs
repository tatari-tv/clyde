//! Pure, deterministic math over a session's signals: token/cost aggregation, the cache-efficiency
//! ratios, and (Phase 3) the behavioral counters and turn-duration percentiles (design "Signals
//! (full scope)" / "Data Model", `docs/design/2026-07-22-session-efficiency-signals.md`).
//!
//! This module owns the two scope-level types the whole crate folds over -- [`RawCounters`]
//! (additive, summable across scopes) and [`EfficiencySignals`] (counters + the metrics DERIVED
//! from them for that scope). The Aggregation invariant lives here: [`RawCounters::merge`] unions
//! additive counters and the duration sample, and [`finalize`] recomputes every derived metric
//! (ratios, percentiles) from the unioned counters -- never by averaging or field-summing sub-scope
//! derived values. `extract`/`fold` populate and union these; `metrics` never touches the
//! filesystem.

use std::collections::BTreeMap;

use claude_pricing::{AssistantEntry, TokenUsage, calculate_usd};
use log::{debug, warn};

/// Tokens + `$` attributed to one workflow bucket (a skill or an MCP tool). Additive across the
/// records that carry the same attribution key, and across scopes in [`RawCounters::merge`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkloadCost {
    pub tokens: u64,
    pub cost_usd: f64,
}

impl WorkloadCost {
    fn merge(&mut self, other: &WorkloadCost) {
        self.tokens += other.tokens;
        self.cost_usd += other.cost_usd;
    }
}

/// Which mechanism triggered a context compaction. `auto` = Claude Code ran the context to the wall
/// and compacted on its own (a signal the session was oversized); `manual` = the user invoked it.
/// A typed vocabulary, not free strings (house typed-values rule); an unrecognized `trigger` string
/// is warn-and-skipped by the extractor rather than coerced into either variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    Auto,
    Manual,
}

impl CompactionTrigger {
    /// Parse the `compactMetadata.trigger` string. Handles BOTH live values (`auto`, and the
    /// `manual` shape the Phase 0 fixtures synthesize since it never occurred in the sampled
    /// corpus). Any other value yields `None` -> the extractor skips that compaction, never
    /// fabricating a trigger.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// One `compact_boundary` event: the trigger, the tokens before/after the compaction, and the dead
/// wall-clock it cost. `pre_tokens - post_tokens` is the reclaimed context.
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    pub trigger: CompactionTrigger,
    pub pre_tokens: u64,
    pub post_tokens: u64,
    pub duration_ms: u64,
}

/// Raw additive counters for one scope (a parent transcript, a subagent, or the whole-session
/// aggregate). Everything here is summable across scopes by [`merge`](RawCounters::merge): scalar
/// counters add, the `turn_durations_ms` SAMPLE and the `compactions` list concatenate, and the
/// attribution maps merge key-wise. The derived metrics ([`EfficiencySignals`]) are NEVER stored
/// here -- they are recomputed from these counters by [`finalize`], so nothing can diverge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawCounters {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub cost_usd: f64,
    /// Count of assistant turns (billable assistant records), per Phase 2. Distinct from
    /// `turn_durations_ms.len()`, which counts `turn_duration` system records.
    pub turns: u64,
    /// The turn-duration SAMPLE (ms), not percentiles -- percentiles do not sum, so the aggregate
    /// recomputes them from the UNIONED sample (Aggregation invariant).
    pub turn_durations_ms: Vec<u64>,
    pub compactions: Vec<Compaction>,
    /// `tool_result` blocks with `is_error == true` (the only sound tool-failure predicate).
    pub tool_errors: u64,
    /// Subset of `tool_errors` whose top-level `toolUseResult` matches the `Error: Exit code N`
    /// Bash-failure shape. ALWAYS `<= tool_errors` (a strict subset, never an independent count).
    pub bash_command_failures: u64,
    /// `toolUseResult.interrupted == true` (structured interrupt).
    pub interrupts_structured: u64,
    /// `[Request interrupted by user]` / `... for tool use` user-text markers.
    pub interrupts_text: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub effort_high: u64,
    pub effort_xhigh: u64,
    /// `message.model` distribution (count of assistant records per model id).
    pub model_mix: BTreeMap<String, u64>,
    /// Tokens/`$` grouped by `attributionSkill`.
    pub by_skill: BTreeMap<String, WorkloadCost>,
    /// Tokens/`$` grouped by `attributionMcpTool`.
    pub by_mcp_tool: BTreeMap<String, WorkloadCost>,
}

impl RawCounters {
    /// Sum of every token field (mirrors `report::session::TokenTotals::total`).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_5m_write_tokens
            + self.cache_1h_write_tokens
    }

    /// Fold one assistant turn's usage into these counters, returning the `$` this turn cost so the
    /// caller can attribute it (to a skill/MCP bucket) without recomputing. `cost_usd` uses the
    /// existing `claude_pricing::calculate_usd` (no new cost math, per the design's Non-Goals); an
    /// unpriced model contributes 0 to `cost_usd` and is logged once per occurrence, matching the
    /// house skip-and-warn pattern in `cost::lib`/`report::report`.
    pub fn add_usage(&mut self, model: &str, usage: &TokenUsage) -> f64 {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.cache_read_tokens += usage.cache_read_tokens;
        self.cache_5m_write_tokens += usage.cache_5m_write_tokens;
        self.cache_1h_write_tokens += usage.cache_1h_write_tokens;
        self.turns += 1;
        *self.model_mix.entry(model.to_string()).or_default() += 1;
        match calculate_usd(model, usage) {
            Ok(cost) => {
                self.cost_usd += cost;
                cost
            }
            Err(e) => {
                warn!("metrics::add_usage: unpriced model `{model}`, contributing $0 to cost_usd: {e}");
                0.0
            }
        }
    }

    /// Phase 2 seam: fold a parsed `claude_pricing::AssistantEntry` (as returned by
    /// `parse_jsonl_file`) into these counters. Delegates to [`add_usage`](Self::add_usage) so token
    /// and cost accumulation has exactly ONE code path shared with Phase 3's per-record extractor.
    pub fn add_entry(&mut self, entry: &AssistantEntry) {
        let _ = self.add_usage(&entry.model, &entry.usage);
    }

    /// Union `other` into `self` (the Aggregation invariant's additive step): scalar counters add,
    /// the duration sample and the compaction list concatenate, and the attribution/model maps
    /// merge key-wise. Derived metrics are NOT merged here -- the caller recomputes them from the
    /// unioned counters via [`finalize`].
    pub fn merge(&mut self, other: &RawCounters) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_5m_write_tokens += other.cache_5m_write_tokens;
        self.cache_1h_write_tokens += other.cache_1h_write_tokens;
        self.cost_usd += other.cost_usd;
        self.turns += other.turns;
        self.turn_durations_ms.extend_from_slice(&other.turn_durations_ms);
        self.compactions.extend_from_slice(&other.compactions);
        self.tool_errors += other.tool_errors;
        self.bash_command_failures += other.bash_command_failures;
        self.interrupts_structured += other.interrupts_structured;
        self.interrupts_text += other.interrupts_text;
        self.web_search_requests += other.web_search_requests;
        self.web_fetch_requests += other.web_fetch_requests;
        self.effort_high += other.effort_high;
        self.effort_xhigh += other.effort_xhigh;
        for (model, count) in &other.model_mix {
            *self.model_mix.entry(model.clone()).or_default() += count;
        }
        for (skill, wc) in &other.by_skill {
            self.by_skill.entry(skill.clone()).or_default().merge(wc);
        }
        for (tool, wc) in &other.by_mcp_tool {
            self.by_mcp_tool.entry(tool.clone()).or_default().merge(wc);
        }
    }
}

/// Counters + the derived metrics computed FOR THAT SCOPE from its own counters (design "Data
/// Model"). Every field here is a pure function of [`RawCounters`]; [`finalize`] is the single
/// place that computes them, so a scope's derived values can never drift from its raw counters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EfficiencySignals {
    pub raw: RawCounters,
    pub cache_read_share: Option<f64>,
    pub cache_1h_write_fraction: Option<f64>,
    pub tokens_per_turn: Option<f64>,
    pub cost_per_turn_usd: Option<f64>,
    pub turn_ms_p50: Option<u64>,
    pub turn_ms_p90: Option<u64>,
    pub turn_ms_max: Option<u64>,
}

/// Compute every derived metric for a scope from its `RawCounters`. This is the ONE recompute path:
/// per-scope finalization and the aggregate finalization both call it, so the aggregate is exactly
/// `finalize(union of all scopes' counters)` and nothing is a stored redundant field (Aggregation
/// invariant). An all-zero scope yields every derived field `None` (never `NaN`).
pub fn finalize(raw: RawCounters) -> EfficiencySignals {
    let (turn_ms_p50, turn_ms_p90, turn_ms_max) = turn_duration_percentiles(&raw.turn_durations_ms);
    let signals = EfficiencySignals {
        cache_read_share: cache_read_share(&raw),
        cache_1h_write_fraction: cache_1h_write_fraction(&raw),
        tokens_per_turn: tokens_per_turn(&raw),
        cost_per_turn_usd: cost_per_turn_usd(&raw),
        turn_ms_p50,
        turn_ms_p90,
        turn_ms_max,
        raw,
    };
    debug!(
        "finalize: turns={} total-tokens={} cost-usd={} cache-read-share={:?} tool-errors={} \
         bash-failures={} compactions={} durations={} p50={:?} p90={:?} max={:?}",
        signals.raw.turns,
        signals.raw.total_tokens(),
        signals.raw.cost_usd,
        signals.cache_read_share,
        signals.raw.tool_errors,
        signals.raw.bash_command_failures,
        signals.raw.compactions.len(),
        signals.raw.turn_durations_ms.len(),
        signals.turn_ms_p50,
        signals.turn_ms_p90,
        signals.turn_ms_max,
    );
    signals
}

/// Sum `entries` (one scope's `AssistantEntry`s from `claude_pricing::parse_jsonl_file`) into
/// `RawCounters`, then [`finalize`] the derived metrics. Phase 2's token-only aggregation seam:
/// the behavioral counters stay at their `Default` (zero) since `AssistantEntry` carries none of
/// them, so `turn_ms_*`/`tool_errors`/etc. are all `None`/0 here. Phase 3's [`crate::extract`]
/// populates the full counter set from its own per-record parse.
pub fn aggregate_tokens(entries: &[AssistantEntry]) -> EfficiencySignals {
    debug!("aggregate_tokens: entries={}", entries.len());
    let mut raw = RawCounters::default();
    for entry in entries {
        raw.add_entry(entry);
    }
    finalize(raw)
}

/// `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)` -- the SAME formula and
/// name as `report::aggregate`'s cache stats, via the shared `common::cache_read_share` helper
/// (design: "siblings behave identically", one definition, no drift). `None` only when the
/// denominator is 0 (a scope with zero assistant tokens); a scope with cache writes but zero reads
/// evaluates to `Some(0.0)` (real waste), never `None`.
pub fn cache_read_share(raw: &RawCounters) -> Option<f64> {
    common::cache_read_share(
        raw.input_tokens,
        raw.cache_read_tokens,
        raw.cache_5m_write_tokens,
        raw.cache_1h_write_tokens,
    )
}

/// `cache_1h_write / (cache_5m_write + cache_1h_write)`. `None` when the scope wrote no cache at
/// all (denominator 0) -- there's no write mix to have a fraction of.
pub fn cache_1h_write_fraction(raw: &RawCounters) -> Option<f64> {
    let denom = raw.cache_5m_write_tokens + raw.cache_1h_write_tokens;
    if denom == 0 {
        None
    } else {
        Some(raw.cache_1h_write_tokens as f64 / denom as f64)
    }
}

fn tokens_per_turn(raw: &RawCounters) -> Option<f64> {
    if raw.turns == 0 {
        None
    } else {
        Some(raw.total_tokens() as f64 / raw.turns as f64)
    }
}

fn cost_per_turn_usd(raw: &RawCounters) -> Option<f64> {
    if raw.turns == 0 { None } else { Some(raw.cost_usd / raw.turns as f64) }
}

/// p50/p90/max of a turn-duration sample (ms), using the nearest-rank method (`ceil(p * n)`-th
/// value, 1-indexed). Recomputed from the UNIONED sample at the aggregate (percentiles do not sum).
/// An empty sample yields `(None, None, None)`.
fn turn_duration_percentiles(sample: &[u64]) -> (Option<u64>, Option<u64>, Option<u64>) {
    if sample.is_empty() {
        return (None, None, None);
    }
    let mut sorted = sample.to_vec();
    sorted.sort_unstable();
    let p50 = nearest_rank(&sorted, 0.50);
    let p90 = nearest_rank(&sorted, 0.90);
    let max = sorted.last().copied();
    (p50, p90, max)
}

/// Nearest-rank percentile of an already-sorted, non-empty slice: the `ceil(p * n)`-th value
/// (1-indexed), clamped to the last index. `p` is in `[0, 1]`.
fn nearest_rank(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    let rank = (p * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted.get(idx).copied()
}

#[cfg(test)]
mod tests;
