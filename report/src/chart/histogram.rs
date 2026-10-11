//! Session-cost histogram over fixed, roughly logarithmic cost buckets.
//!
//! Session spend is heavy-tailed (a September window: median $0.52, 90th percentile $20), so equal-
//! width buckets put nearly every session in the first bar. The bucket edges step 1, 2.5, 5 per
//! decade, which keeps a fifty-cent session and a twenty-dollar session both legible. Edges are
//! whole cents, the unit session spend reconciles in.

use super::scale;
use log::debug;

/// Bucket lower edges in cents, ascending; bucket `i` is `[EDGES[i], EDGES[i + 1])` and the last
/// is open-ended.
pub const EDGES: [i64; 14] = [
    0, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000,
];

/// Shortest column drawn for a nonzero count, in tenths of the plot height, so a bucket with one
/// session among hundreds still shows.
pub const MIN_VISIBLE: i64 = 15;

/// The occupied run of buckets: from the lowest occupied bucket to the highest, inclusive, with
/// interior empties kept so the x axis stays a continuous cost scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// Index into [`EDGES`] of the first bucket shown.
    pub first: usize,
    /// Sessions per shown bucket.
    pub counts: Vec<usize>,
    /// Column heights, tenths of the plot height, scaled against the largest count.
    pub heights: Vec<i64>,
}

/// The bucket a spend in cents falls in. A negative spend falls in the first bucket.
pub fn bucket_of(cents: i64) -> usize {
    EDGES.iter().rposition(|edge| cents >= *edge).unwrap_or(0)
}

/// `(lower, upper)` cents of bucket `index`; `upper` is `None` for the open last bucket.
pub fn bounds(index: usize) -> (i64, Option<i64>) {
    (EDGES[index], EDGES.get(index + 1).copied())
}

/// Bucket `values` (cents). `None` when there are no values.
pub fn histogram(values: &[i64]) -> Option<Histogram> {
    debug!("chart::histogram::histogram: values={}", values.len());
    let mut counts = [0usize; EDGES.len()];
    for value in values {
        counts[bucket_of(*value)] += 1;
    }
    let first = counts.iter().position(|c| *c > 0)?;
    let last = counts.iter().rposition(|c| *c > 0)?;
    let shown = counts[first..=last].to_vec();
    let max = shown.iter().copied().max().unwrap_or(0) as f64;
    let heights = shown
        .iter()
        .map(|c| match c {
            0 => 0,
            c => scale(*c as f64, max).max(MIN_VISIBLE),
        })
        .collect();
    debug!("chart::histogram::histogram: buckets {first}..={last} max-count={max}");
    Some(Histogram {
        first,
        counts: shown,
        heights,
    })
}

#[cfg(test)]
mod tests;
