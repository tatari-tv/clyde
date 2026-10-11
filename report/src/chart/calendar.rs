//! A calendar heatmap: one square per day, Monday-first weeks as rows, shaded by the day's value.
//!
//! Month-calendar orientation (seven columns, one row per week) rather than a contribution graph's
//! weeks-as-columns: a report covers a month, and a month reads as a calendar. Coordinates are
//! whole user units in a viewBox exactly as wide as seven cells.

use chrono::{Datelike, NaiveDate};
use log::debug;

/// One cell's side and the gap between cells, in user units.
pub const CELL: i64 = 10;
pub const GAP: i64 = 2;

/// Shade levels above zero. Level `0` is a day with nothing; `1..=LEVELS` split `(0, max]` evenly.
pub const LEVELS: i64 = 4;

/// Width of the grid, in user units: seven cells and six gaps.
pub const WIDTH: i64 = 7 * CELL + 6 * GAP;

/// One day's square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub x: i64,
    pub y: i64,
    /// `0..=LEVELS`.
    pub level: i64,
}

/// The laid-out calendar: one cell per input day, in order, and the grid's height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    pub height: i64,
    pub cells: Vec<Cell>,
}

/// Lay out consecutive days starting at `first`, one value per day. `None` when there are no days
/// or no day has a positive value: an all-blank calendar says nothing a sentence does not.
pub fn calendar(first: NaiveDate, values: &[f64]) -> Option<Calendar> {
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    debug!(
        "chart::calendar::calendar: first={first} days={} max={max}",
        values.len()
    );
    if max <= 0.0 {
        debug!("chart::calendar::calendar: absent, no day has a positive value");
        return None;
    }
    let lead = first.weekday().num_days_from_monday() as i64;
    let step = CELL + GAP;
    let cells: Vec<Cell> = values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let slot = lead + i as i64;
            Cell {
                x: slot % 7 * step,
                y: slot / 7 * step,
                level: level(*value, max),
            }
        })
        .collect();
    let rows = (lead + values.len() as i64 + 6) / 7;
    Some(Calendar {
        height: rows * step - GAP,
        cells,
    })
}

/// `0` for a non-positive value, else `ceil(value / max * LEVELS)`, so the max is always the
/// darkest shade and any positive day is at least the lightest.
fn level(value: f64, max: f64) -> i64 {
    if value <= 0.0 {
        return 0;
    }
    ((value / max * LEVELS as f64).ceil() as i64).clamp(1, LEVELS)
}

#[cfg(test)]
mod tests;
