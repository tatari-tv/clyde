//! Paired share bars: for each row, two shares (of sessions, of spend) drawn against ONE scale.
//!
//! One scale for both series is the point of the chart: a row whose spend bar is longer than its
//! sessions bar costs more than its share of the work, and that comparison only reads true when a
//! percent is the same length in both series. The largest share in either series fills the track.

use super::{FULL, scale};
use log::debug;

/// Shortest bar drawn for a nonzero share, in tenths of the track. A share that rounds to a hair
/// would otherwise vanish and read as zero; the value text beside it still states the real figure.
pub const MIN_VISIBLE: i64 = 6;

/// One row's two bar lengths, in tenths of the track (`0..=FULL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub first: i64,
    pub second: i64,
}

/// Bar lengths for each `(first, second)` share pair, scaled against the largest share in either
/// series. `None` when there are no rows or every share is zero: there is nothing to scale against.
pub fn paired(shares: &[(f64, f64)]) -> Option<Vec<Pair>> {
    let max = shares.iter().flat_map(|(a, b)| [*a, *b]).fold(0.0_f64, f64::max);
    debug!("chart::bars::paired: rows={} max-share={max}", shares.len());
    if max <= 0.0 {
        debug!("chart::bars::paired: absent, no positive share");
        return None;
    }
    Some(
        shares
            .iter()
            .map(|(a, b)| Pair {
                first: length(*a, max),
                second: length(*b, max),
            })
            .collect(),
    )
}

/// One bar's length: scaled, floored at [`MIN_VISIBLE`] when the share is positive, capped at
/// [`FULL`].
fn length(share: f64, max: f64) -> i64 {
    if share <= 0.0 {
        return 0;
    }
    scale(share, max).clamp(MIN_VISIBLE, FULL)
}

#[cfg(test)]
mod tests;
