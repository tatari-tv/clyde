//! Whole cents: the one unit every total a reader sees is summed in.
//!
//! Several tables in a report are partitions: every dollar in the headline lands in exactly one row,
//! and the prose says so. Summing unrounded per-row dollars and rounding each row on its own does
//! not preserve that: the medium fixture once rendered four agent-type rows summing to `$671.27`
//! under a sentence asserting they totaled `$671.28`. An artifact that states an exact total its own
//! visible rows contradict is a rendered falsehood, which is the class this design exists to remove.
//!
//! So every partition is built from the same per-session integer cents the headline is the sum of:
//! a session's cents are final ([`to_cents`] of its rounded spend), a row is a sum of whole session
//! cents (repo, org, band) or of a session's cents split across its parts by [`apportion`] (days,
//! agent types), and integer sums cannot drift. Where a table is NOT a partition (`by-skill` /
//! `by-mcp` are attribution tags; `by-repo` when some session is unattributed) its rows simply sum
//! to what they cover.

use log::{trace, warn};

/// Cents in a dollar. The unit allocation happens in, because it is the unit the artifact displays.
pub(crate) const CENTS_PER_DOLLAR: f64 = 100.0;

/// Round a dollar figure to cents, normalizing negative zero to `+0.0`.
///
/// `-0.0` is not hypothetical. Rust's `Sum for f64` folds from `-0.0`
/// (`core/src/iter/traits/accum.rs:164`), so summing an EMPTY iterator of priced models yields
/// `-0.0`, and `(-0.0 * 100.0).round() / 100.0` preserves the sign all the way into serde. Every
/// zero-session report therefore serialized `"spend-usd": -0.0`, which reads as a broken number to
/// anyone running collect on a fresh catalog.
///
/// This is the ONE copy. It used to live three times (`report`, `reconcile`, `merge`), each module
/// re-normalizing independently, and the copies diverged exactly as copies do: `merge`'s dropped
/// the negative-zero normalization, so a merged artifact could serialize `-0.0` where the other
/// paths could not (issue #67). Every dollar choke point in the crate now rounds here.
pub(crate) fn round_cents(x: f64) -> f64 {
    let cents = (x * CENTS_PER_DOLLAR).round() / CENTS_PER_DOLLAR;
    // `-0.0 == 0.0` is true in IEEE 754, so this catches negative zero with no signum dance, and
    // leaves every other value untouched.
    if cents == 0.0 { 0.0 } else { cents }
}

/// Split `total` whole cents across buckets in proportion to `weights`, by largest remainder, so the
/// parts sum to `total` EXACTLY.
///
/// The daily-spend allocation: a session's cents are already final, and each of its days gets a
/// share in proportion to what that day's tokens cost. Unlike [`allocate`] there is no budget to
/// decline on, because the total is the authority and the weights only say how to divide it.
/// Each bucket gets the floor of its exact share, then the leftover cents go one at a time to the
/// largest discarded fractions, index ascending breaking ties. Negative weights count as zero.
/// When every weight is zero the cents go to the first bucket: they must land somewhere, and the
/// caller orders buckets so the first is the right fallback (a session's begin day). Empty
/// `weights` with a non-zero `total` is the one case that cannot be honored; the caller never
/// builds it, and it returns an empty split with a warning.
pub(crate) fn apportion(total: i64, weights: &[f64]) -> Vec<i64> {
    if weights.is_empty() {
        if total != 0 {
            warn!("cents::apportion: {total} cent(s) with no bucket to carry them");
        }
        return Vec::new();
    }
    let sum: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if sum <= 0.0 {
        let mut out = vec![0; weights.len()];
        out[0] = total;
        trace!("cents::apportion: all-zero weights, {total} cent(s) to the first bucket");
        return out;
    }
    let mut out: Vec<i64> = Vec::with_capacity(weights.len());
    let mut remainders: Vec<(f64, usize)> = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        let exact = total as f64 * weight.max(0.0) / sum;
        let floor = exact.floor();
        out.push(floor as i64);
        remainders.push((exact - floor, index));
    }
    let residual = total - out.iter().sum::<i64>();
    remainders.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    if residual < 0 {
        remainders.reverse();
    }
    let step = if residual < 0 { -1 } else { 1 };
    for (_, index) in remainders.iter().cycle().take(residual.unsigned_abs() as usize) {
        out[*index] += step;
    }
    trace!(
        "cents::apportion: buckets={} total={total} residual={residual}",
        weights.len()
    );
    out
}

/// A dollar figure as whole cents. The unit every band sum is taken in, so bands add up exactly.
pub(crate) fn to_cents(dollars: f64) -> i64 {
    (dollars * CENTS_PER_DOLLAR).round() as i64
}

/// One allocated bucket back in dollars, for `format_usd`.
pub(crate) fn to_dollars(cents: i64) -> f64 {
    cents as f64 / CENTS_PER_DOLLAR
}

#[cfg(test)]
mod tests;
