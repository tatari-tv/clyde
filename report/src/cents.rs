//! Making a DISPLAYED partition sum to its DISPLAYED total.
//!
//! Several tables in a report are partitions: every dollar in the headline lands in exactly one row,
//! and the prose says so. Rounding each row independently does not preserve that. Every row is a
//! real unrounded dollar figure, `format_usd` rounds each to cents on its own, and the cents need
//! not add up: the medium fixture rendered four agent-type rows summing to `$671.27` under a
//! sentence asserting they totaled `$671.28`, and its by-repo table summed to `$671.26`.
//!
//! An artifact that states an exact total its own visible rows contradict is a rendered falsehood,
//! which is the class this design exists to remove. So the residual is allocated BEFORE rendering,
//! by largest remainder, in the unit the reader adds the table up in.
//!
//! This module is deliberately about PRESENTATION only. It never changes a measurement: the raw
//! operands keep their measured values, chart geometry stays proportional to those, and the most any
//! displayed row moves is one cent. Where a table is NOT a partition (`by-skill` / `by-mcp` are
//! attribution tags; `by-repo` when some session is unattributed) the caller must not use this --
//! forcing those to sum would attribute money no row's own arithmetic supports.

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

/// Decimal places of a CENT figure kept before flooring, to absorb accumulated `f64` error.
///
/// Load bearing. Every value here is a sum of per-session dollars, so it carries the usual binary
/// representation error: the pathological fixture's one repo accumulates to `49.46999999999999`,
/// whose `* 100` is `4946.999999999999`, which `floor()` takes to **4946** -- a whole cent below the
/// 4947 it plainly is. That made the residual look like 2 cents across 1 bucket, over the one-cent
/// budget, so the allocator declined and the table stayed a cent adrift from its own headline: the
/// exact defect it exists to fix, caused by the fix.
///
/// Six places is far finer than any real fractional cent and far coarser than the ~1e-12 error being
/// absorbed, so it snaps the artifact away without touching a genuine `.5` remainder.
const CENT_PRECISION: f64 = 1e6;

/// Allocate `total` across `values` in whole cents, by largest remainder.
///
/// Floors every value to a cent, then hands the leftover cents out one at a time to the values with
/// the largest discarded fraction, index ascending breaking ties (callers pass values in display
/// order, so the tiebreak is stable across runs). A shortfall is taken back the same way, from the
/// smallest remainders first. Deterministic: same input, same cents, every run.
///
/// `None` when `values` is empty, or when the gap exceeds ONE CENT PER VALUE -- the most flooring can
/// lose. A larger gap is not a rounding artifact: the rows genuinely disagree with the total, and
/// smearing that across the table would hide a real defect behind a partition that adds up. The
/// caller renders its measured figures instead, and the discrepancy stays visible.
pub(crate) fn allocate(values: &[f64], total: f64) -> Option<Vec<i64>> {
    if values.is_empty() {
        if total.abs() * CENTS_PER_DOLLAR >= 1.0 {
            warn!("cents::allocate: no buckets to carry a {total:.2} total");
        }
        return None;
    }
    let target = (total * CENTS_PER_DOLLAR).round() as i64;
    let mut cents: Vec<i64> = Vec::with_capacity(values.len());
    let mut remainders: Vec<(f64, usize)> = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        // Snap off the accumulated `f64` error BEFORE flooring; see `CENT_PRECISION`.
        let exact = (value * CENTS_PER_DOLLAR * CENT_PRECISION).round() / CENT_PRECISION;
        let floor = exact.floor();
        cents.push(floor as i64);
        remainders.push((exact - floor, index));
    }
    let residual = target - cents.iter().sum::<i64>();
    let budget = values.len() as i64;
    if residual.abs() > budget {
        warn!(
            "cents::allocate: {} bucket(s) sum to {:.2} against a total of {total:.2}; a {} cent gap \
             is more than rounding, so nothing is allocated and the rows will NOT sum",
            values.len(),
            cents.iter().sum::<i64>() as f64 / CENTS_PER_DOLLAR,
            residual.abs()
        );
        return None;
    }

    // Largest remainder first when handing cents OUT, smallest first when taking them back.
    remainders.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    if residual < 0 {
        remainders.reverse();
    }
    let step = if residual < 0 { -1 } else { 1 };
    for (_, index) in remainders.iter().take(residual.unsigned_abs() as usize) {
        cents[*index] += step;
    }
    trace!(
        "cents::allocate: buckets={} residual={residual} target={target}",
        values.len()
    );
    Some(cents)
}

/// One allocated bucket back in dollars, for `format_usd`.
pub(crate) fn to_dollars(cents: i64) -> f64 {
    cents as f64 / CENTS_PER_DOLLAR
}

#[cfg(test)]
mod tests;
