//! List price -> cache savings -> spend, as three horizontal steps on one scale.
//!
//! The list price runs from zero; the savings step hangs from the list price's end back to the
//! spend; the spend runs from zero. When the 1h cache-write premium outweighs the read savings,
//! savings are negative and the step RISES from the list price up to the larger spend, rather than
//! drawing a negative width. Inputs are whole cents, the raw twins of the three display strings, so
//! the reconciliation `list - savings == spend` is exact or the chart is not drawn.

use super::scale;
use log::{debug, warn};

/// One step: its start and width, in tenths of the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub start: i64,
    pub width: i64,
}

/// The three steps, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waterfall {
    pub list: Step,
    pub savings: Step,
    pub spend: Step,
    /// `true` when savings are negative and the middle step rises.
    pub savings_rise: bool,
}

/// The waterfall for `list - savings = spend`, all in cents. `None` when the three do not
/// reconcile (a chart that shows a gap the figures beside it do not), or when there is nothing to
/// draw (no list price and no spend).
pub fn waterfall(list: i64, savings: i64, spend: i64) -> Option<Waterfall> {
    debug!("chart::waterfall::waterfall: list={list} savings={savings} spend={spend}");
    if list - savings != spend {
        warn!("chart::waterfall: list {list} minus savings {savings} is not spend {spend}; not drawn");
        return None;
    }
    let max = list.max(spend) as f64;
    if max <= 0.0 {
        debug!("chart::waterfall::waterfall: absent, nothing to draw");
        return None;
    }
    let low = scale(list.min(spend) as f64, max);
    let high = scale(list.max(spend) as f64, max);
    Some(Waterfall {
        list: Step {
            start: 0,
            width: scale(list as f64, max),
        },
        savings: Step {
            start: low,
            width: high - low,
        },
        spend: Step {
            start: 0,
            width: scale(spend as f64, max),
        },
        savings_rise: savings < 0,
    })
}

#[cfg(test)]
mod tests;
