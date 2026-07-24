//! Shared numeric metrics reused across sibling crates that mine the same session token totals,
//! so a formula lives in exactly ONE place instead of drifting between them (per the "siblings
//! behave identically" house rule). `report::aggregate`'s cache-efficiency rollup and the
//! `efficiency` crate's per-session signals both call [`cache_read_share`] rather than each
//! reimplementing the ratio. See `docs/design/2026-07-22-session-efficiency-signals.md`, Phase 2.
//!
//! [`TokenTotals`] + [`price`] are the Phase 1 lift (`docs/design/2026-07-24-report-collect-once-render-from-data.md`):
//! `report`'s token accumulator and pricing seam, unified here so `report` and `efficiency` share
//! one `add`/`merge`/pricing path. `TokenTotals` carries no dollar field -- pricing is computed
//! ON DEMAND from the accumulated raw counters by [`price`], never folded into the struct itself,
//! so summing/merging totals can never accidentally sum already-priced dollars (the Aggregation
//! invariant, `efficiency/src/metrics.rs:9`, applied to money).

use claude_pricing::{Pricing, TokenUsage};

/// `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)`.
///
/// `None` only when the denominator is 0 (a scope with zero assistant tokens at all). A scope
/// with cache writes but zero reads evaluates to `Some(0.0)` (real cache waste), never `None` --
/// callers must not conflate "no cache activity to measure" with "measured and it's zero".
pub fn cache_read_share(input: u64, cache_read: u64, cache_5m_write: u64, cache_1h_write: u64) -> Option<f64> {
    let denom = input + cache_read + cache_5m_write + cache_1h_write;
    if denom == 0 { None } else { Some(cache_read as f64 / denom as f64) }
}

/// Additive raw token counters for one scope (a model bucket, a session, a whole report). Every
/// field here is summable across scopes by [`merge`](TokenTotals::merge) -- this struct carries
/// NO dollar field, so pricing can never be field-summed by accident; [`price`] is the only path
/// from a `TokenTotals` to a `$` figure, and it is always called LAST, after every record has
/// already been folded in via [`add`](TokenTotals::add)/`merge`.
///
/// Lifted from `report::session::TokenTotals` (Phase 1); `report` re-exports this type from
/// `report::session` so existing call sites are unaffected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
}

impl TokenTotals {
    /// Fold one record's usage in. `total` is recomputed from the other five fields on every
    /// call, never carried forward independently -- it can't drift from its own components.
    pub fn add(&mut self, usage: &TokenUsage) {
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;
        self.cache_5m_write += usage.cache_5m_write_tokens;
        self.cache_1h_write += usage.cache_1h_write_tokens;
        self.cache_read += usage.cache_read_tokens;
        self.total = self.input + self.output + self.cache_5m_write + self.cache_1h_write + self.cache_read;
    }

    /// Union `other` into `self` (the additive step of the Aggregation invariant): every field
    /// adds, `total` is recomputed, and no derived/priced value is touched (there is none here).
    pub fn merge(&mut self, other: &TokenTotals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_5m_write += other.cache_5m_write;
        self.cache_1h_write += other.cache_1h_write;
        self.cache_read += other.cache_read;
        self.total = self.input + self.output + self.cache_5m_write + self.cache_1h_write + self.cache_read;
    }

    /// Recast the accumulated totals as a single `TokenUsage`, the shape [`price`] (and
    /// `claude_pricing::calculate_cost` underneath it) expects.
    pub fn as_usage(&self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cache_5m_write_tokens: self.cache_5m_write,
            cache_1h_write_tokens: self.cache_1h_write,
            cache_read_tokens: self.cache_read,
        }
    }
}

/// Price `usage` for `model` against the caller-supplied `pricing` source, returning `None` (never
/// panicking) when `model` has no entry in that source -- graceful degradation for a historical
/// model retired from `pricing.yml` (report's pre-lift contract, `report.rs:89`). `pricing` is an
/// explicit parameter rather than a fixed global because `report` and `efficiency` deliberately
/// price from DIFFERENT sources:
///
/// - `report` prices via a live/fetched `Pricing` (`Pricing::auto`, `lib.rs:139`) so a report
///   reflects the current feed.
/// - `efficiency`'s catalog reindex path prices via `Pricing::embedded()` so a session's stored
///   `cost_usd` is deterministic and reproducible from the same JSONL regardless of network state
///   or feed staleness at reindex time -- a catalog value must not silently change on a later
///   reindex just because the feed moved.
///
/// `pricing.calculate_usd` already logs its own `warn!` on an unpriced model (`claude-pricing`
/// crate); this function does not duplicate that log, it only translates the `Result` into the
/// `Option` shape both callers build their "$0 + flag" degradation on.
pub fn price(model: &str, usage: &TokenUsage, pricing: &Pricing) -> Option<f64> {
    pricing.calculate_usd(model, usage).ok()
}

#[cfg(test)]
mod tests;
