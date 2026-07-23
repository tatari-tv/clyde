//! Pure, deterministic math over a session's parsed `AssistantEntry`s: token/cost aggregation and
//! the cache-efficiency ratios (design "Signals (full scope)" / "Data Model",
//! `docs/design/2026-07-22-session-efficiency-signals.md`, Phase 2).
//!
//! Phase 2 populates only the token/cost fields the design's `RawCounters`/`EfficiencySignals`
//! need for per-session aggregation. The behavioral counters (`tool_errors`, `compactions`,
//! `interrupts_*`, `effort_*`, `model_mix`, `by_skill`, `by_mcp_tool`) and the turn-duration
//! percentiles land in Phase 3 once the behavioral extractor exists to populate them -- adding
//! those fields here now would be dead weight nothing writes to yet.

use claude_pricing::{AssistantEntry, calculate_usd};
use log::{debug, warn};

/// Additive token/cost counters for one scope (a session, a file, a subagent -- see the design
/// doc's Aggregation invariant). Summable across scopes by simple field addition; a future phase
/// recomputes the derived ratios from summed `RawCounters`, never averages sub-scope ratios.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawCounters {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
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

    /// Fold one assistant turn's usage into these counters. `cost_usd` uses the existing
    /// `claude_pricing::calculate_usd` (no new cost math, per the design's Non-Goals); an unpriced
    /// model contributes 0 to `cost_usd` and is logged once per occurrence, matching the house
    /// pattern in `cost::lib`/`report::report` (skip-and-warn, never a hard failure over one
    /// unknown model id).
    fn add_entry(&mut self, entry: &AssistantEntry) {
        self.input_tokens += entry.usage.input_tokens;
        self.output_tokens += entry.usage.output_tokens;
        self.cache_read_tokens += entry.usage.cache_read_tokens;
        self.cache_5m_write_tokens += entry.usage.cache_5m_write_tokens;
        self.cache_1h_write_tokens += entry.usage.cache_1h_write_tokens;
        self.turns += 1;
        match calculate_usd(&entry.model, &entry.usage) {
            Ok(cost) => self.cost_usd += cost,
            Err(e) => warn!(
                "metrics::add_entry: unpriced model `{}`, contributing $0 to cost_usd: {}",
                entry.model, e
            ),
        }
    }
}

/// Counters + the derived metrics computed FOR THAT SCOPE from its own counters (design "Data
/// Model"). `turn_ms_p50`/`turn_ms_p90`/`turn_ms_max` are added in Phase 3 alongside
/// turn-duration extraction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EfficiencySignals {
    pub raw: RawCounters,
    pub cache_read_share: Option<f64>,
    pub cache_1h_write_fraction: Option<f64>,
    pub tokens_per_turn: Option<f64>,
    pub cost_per_turn_usd: Option<f64>,
}

/// Sum `entries` (one scope's `AssistantEntry`s from `claude_pricing::parse_jsonl_file`) into
/// `RawCounters`, then derive the ratio/per-turn metrics from the summed totals. An empty slice
/// yields all-zero counters and every derived field `None` (never `NaN`).
pub fn aggregate_tokens(entries: &[AssistantEntry]) -> EfficiencySignals {
    debug!("aggregate_tokens: entries={}", entries.len());
    let mut raw = RawCounters::default();
    for entry in entries {
        raw.add_entry(entry);
    }
    let signals = EfficiencySignals {
        cache_read_share: cache_read_share(&raw),
        cache_1h_write_fraction: cache_1h_write_fraction(&raw),
        tokens_per_turn: tokens_per_turn(&raw),
        cost_per_turn_usd: cost_per_turn_usd(&raw),
        raw,
    };
    debug!(
        "aggregate_tokens: turns={} total-tokens={} cost-usd={} cache-read-share={:?}",
        signals.raw.turns,
        signals.raw.total_tokens(),
        signals.raw.cost_usd,
        signals.cache_read_share
    );
    signals
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

#[cfg(test)]
mod tests;
