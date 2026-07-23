//! Shared numeric metrics reused across sibling crates that mine the same session token totals,
//! so a formula lives in exactly ONE place instead of drifting between them (per the "siblings
//! behave identically" house rule). `report::aggregate`'s cache-efficiency rollup and the
//! `efficiency` crate's per-session signals both call [`cache_read_share`] rather than each
//! reimplementing the ratio. See `docs/design/2026-07-22-session-efficiency-signals.md`, Phase 2.

/// `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)`.
///
/// `None` only when the denominator is 0 (a scope with zero assistant tokens at all). A scope
/// with cache writes but zero reads evaluates to `Some(0.0)` (real cache waste), never `None` --
/// callers must not conflate "no cache activity to measure" with "measured and it's zero".
pub fn cache_read_share(input: u64, cache_read: u64, cache_5m_write: u64, cache_1h_write: u64) -> Option<f64> {
    let denom = input + cache_read + cache_5m_write + cache_1h_write;
    if denom == 0 { None } else { Some(cache_read as f64 / denom as f64) }
}

#[cfg(test)]
mod tests;
