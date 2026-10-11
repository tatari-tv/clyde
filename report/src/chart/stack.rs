//! One bar split into segments, one per value, that tile the bar exactly.
//!
//! Segment edges come from the running total, rounded once per edge, so neighbouring segments share
//! an edge and the widths always sum to [`FULL`]: no hairline gap, no overlap, no matter how the
//! individual widths round.

use super::FULL;
use log::debug;

/// Narrowest segment, in tenths of the bar, that carries an inline label. A narrower segment keeps
/// its tooltip and its legend row; its label would not fit.
pub const MIN_LABEL_WIDTH: i64 = 90;

/// One segment: its left edge and width, in tenths of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub x: i64,
    pub w: i64,
}

impl Segment {
    /// `true` when the segment is wide enough to carry an inline label.
    pub fn labeled(&self) -> bool {
        self.w >= MIN_LABEL_WIDTH
    }
}

/// Segments for `values`, in order. A negative value counts as zero (a zero-width segment).
/// `None` when there are no values or they sum to nothing.
pub fn stack(values: &[f64]) -> Option<Vec<Segment>> {
    let total: f64 = values.iter().map(|v| v.max(0.0)).sum();
    debug!("chart::stack::stack: values={} total={total}", values.len());
    if total <= 0.0 {
        debug!("chart::stack::stack: absent, nothing to stack");
        return None;
    }
    let mut running = 0.0_f64;
    let mut left = 0_i64;
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        running += value.max(0.0);
        let right = (running / total * FULL as f64).round() as i64;
        out.push(Segment {
            x: left,
            w: right - left,
        });
        left = right;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
