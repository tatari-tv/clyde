//! `--worst N` ranking: sort sessions by cache-waste severity (ascending `cache_read_share`), with
//! `None` (a session whose denominator was 0 -- no assistant tokens at all) sorted LAST, never
//! treated as "worst" (design "API Design": "`None` cache-read-share ... sorts LAST (not as
//! worst)"). A write-but-no-read session (`Some(0.0)`) sorts FIRST -- genuinely worst, real cache
//! waste, never confused with the "nothing to measure" case.

use std::cmp::Ordering;

use log::debug;

use crate::collect::CollectedSession;

/// The worst `n` sessions by `cache_read_share`, ascending (lowest/worst share first). `None`-share
/// sessions sort after every computable `Some` share, so they are excluded from the ranked head
/// unless `n` exceeds the number of sessions with a computable share.
pub fn worst(mut sessions: Vec<CollectedSession>, n: usize) -> Vec<CollectedSession> {
    debug!("rank::worst: sessions={} n={n}", sessions.len());
    sessions.sort_by(|a, b| {
        compare_cache_share(
            a.efficiency.aggregate.cache_read_share,
            b.efficiency.aggregate.cache_read_share,
        )
    });
    sessions.truncate(n);
    sessions
}

/// `None` sorts as [`Ordering::Greater`] against every `Some` -- it always sorts LAST, never
/// competing against a real (possibly `0.0`, genuinely worst) share for the ranked head.
fn compare_cache_share(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests;
