//! One bar split into segments, one per value, that tile the bar exactly; and a row of such bars
//! standing as columns on one shared scale (daily spend stacked by model).
//!
//! Segment edges come from the running total, rounded once per edge, so neighbouring segments share
//! an edge and the widths always sum to [`FULL`]: no hairline gap, no overlap, no matter how the
//! individual widths round. Columns use the same running-edge rule against the tallest column.

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
    Some(edges(values, total))
}

/// Segments whose edges are the running total of `values` as tenths of `scale`.
fn edges(values: &[f64], scale: f64) -> Vec<Segment> {
    let mut running = 0.0_f64;
    let mut left = 0_i64;
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        running += value.max(0.0);
        let right = (running / scale * FULL as f64).round() as i64;
        out.push(Segment {
            x: left,
            w: right - left,
        });
        left = right;
    }
    out
}

/// One standing column of a stacked column chart: its slot on the x axis and its blocks, all in
/// tenths of the plot ([`FULL`] wide and [`FULL`] tall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Left edge.
    pub x: i64,
    pub w: i64,
    /// One block per value, bottom first. Zero-height blocks are kept so block `i` is value `i`.
    pub blocks: Vec<Block>,
}

/// One block of a column: `y` is its TOP edge measured down from the top of the plot (SVG
/// orientation), `h` its height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub y: i64,
    pub h: i64,
}

/// Columns for a series of stacked days: `days[i][k]` is value `k` on day `i`. Every column stands
/// on one scale, the tallest day's total, so heights compare across days; within a column the block
/// edges come from the running total, so the blocks tile the column with no gap. Each column takes
/// an equal slot of the width, inset by an eighth on both sides so neighbours never touch.
/// `None` when there are no days or no day has a positive total.
pub fn columns(days: &[Vec<f64>]) -> Option<Vec<Column>> {
    let max = days
        .iter()
        .map(|values| values.iter().map(|v| v.max(0.0)).sum::<f64>())
        .fold(0.0_f64, f64::max);
    debug!("chart::stack::columns: days={} max={max}", days.len());
    if max <= 0.0 {
        debug!("chart::stack::columns: absent, no day has anything to stack");
        return None;
    }
    let n = days.len() as f64;
    let out = days
        .iter()
        .enumerate()
        .map(|(i, values)| {
            let left = (i as f64 / n * FULL as f64).round() as i64;
            let right = ((i + 1) as f64 / n * FULL as f64).round() as i64;
            let inset = (right - left) / 8;
            let blocks = edges(values, max)
                .into_iter()
                .map(|seg| Block {
                    y: FULL - seg.x - seg.w,
                    h: seg.w,
                })
                .collect();
            Column {
                x: left + inset,
                w: right - left - 2 * inset,
                blocks,
            }
        })
        .collect();
    Some(out)
}

#[cfg(test)]
mod tests;
