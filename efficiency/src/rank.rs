//! `--worst N` ranking: sort sessions by cache-waste severity (ascending `cache_read_share`),
//! honoring the SAME eligibility gate the flagging path uses ([`crate::score::is_eligible`]).
//!
//! Three ordering tiers (design "API Design" + Phase 5 acceptance criterion, "three lowest-ELIGIBLE-
//! share sessions"):
//! - ELIGIBLE with a computable share -- the genuine cache-waste candidates, sorted ascending
//!   (lowest/worst share first). A write-but-no-read session (`Some(0.0)`) is real waste and sorts
//!   FIRST.
//! - INELIGIBLE (a short one-shot that cannot structurally reuse cache) -- excluded from the worst
//!   head, sorted AFTER every eligible session, so a structurally-low share never masquerades as
//!   "worst" (the exact false positive the eligibility gate exists to kill).
//! - `None` share (denominator 0 -- no assistant tokens at all, nothing to measure) -- always LAST.

use std::cmp::Ordering;

use common::EfficiencyConfig;
use log::debug;

use crate::collect::CollectedSession;
use crate::score::is_eligible;

/// The worst `n` sessions by cache-waste severity, honoring the eligibility gate in `config`.
/// Eligible-with-a-share sessions rank first (ascending share); ineligible sessions sort after
/// them; `None`-share sessions sort last. So `--worst n` returns the n lowest ELIGIBLE shares,
/// only spilling into ineligible/`None` sessions when `n` exceeds the eligible pool.
pub fn worst(mut sessions: Vec<CollectedSession>, n: usize, config: &EfficiencyConfig) -> Vec<CollectedSession> {
    debug!("rank::worst: sessions={} n={n}", sessions.len());
    sessions.sort_by(|a, b| worst_ordering(a, b, config));
    sessions.truncate(n);
    sessions
}

/// Order two sessions worst-first: by tier (eligible-share < ineligible-share < `None`), then within
/// a tier by ascending `cache_read_share` (lowest = worst).
fn worst_ordering(a: &CollectedSession, b: &CollectedSession, config: &EfficiencyConfig) -> Ordering {
    let (tier_a, share_a) = rank_key(a, config);
    let (tier_b, share_b) = rank_key(b, config);
    tier_a.cmp(&tier_b).then_with(|| share_a.total_cmp(&share_b))
}

/// `(tier, share-for-tiebreak)`. Lower tier ranks worse (first). Tier `0` = eligible with a
/// computable share (the ranked head), `1` = ineligible with a computable share (excluded from the
/// head), `2` = `None` share (always last; its `0.0` tiebreak never competes across tiers).
fn rank_key(session: &CollectedSession, config: &EfficiencyConfig) -> (u8, f64) {
    let aggregate = &session.efficiency.aggregate;
    match aggregate.cache_read_share {
        Some(share) if is_eligible(aggregate, config) => (0, share),
        Some(share) => (1, share),
        None => (2, 0.0),
    }
}

#[cfg(test)]
mod tests;
