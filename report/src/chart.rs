//! Chart geometry: pure arithmetic from raw view-model numbers to coordinates.
//!
//! Every module here returns geometry only, as whole numbers: either whole SVG user units or
//! TENTHS (of a user unit, or of a percent of a track), which [`tenths`] writes as a fixed
//! one-decimal string. No module here formats a figure a reader reads: labels are display strings
//! the view model formats with [`crate::fmt`], and a backend writes geometry and labels through
//! separate paths (`render/html/page.rs`).
//!
//! - [`line`]: the by-day polyline (markdown and HTML).
//! - [`bars`]: paired share bars, two series against one scale.
//! - [`stack`]: one bar split into segments that tile it exactly.
//! - [`treemap`]: squarified two-level treemap.
//! - [`waterfall`]: list price, cache savings, spend.
//! - [`histogram`]: counts over fixed log-ish cost buckets.
//! - [`calendar`]: a Monday-first month calendar of daily cells.

pub mod bars;
pub mod calendar;
pub mod histogram;
pub mod line;
pub mod stack;
pub mod treemap;
pub mod waterfall;

pub use line::{BASELINE_Y, Charts, GRIDLINE_YS, LineChart, PLOT_RIGHT, VIEWBOX, compute_charts};

/// A whole track in tenths of a percent: `1000` is `100.0%`.
pub const FULL: i64 = 1000;

/// `value / max` of [`FULL`], rounded to the nearest tenth of a percent. `0` when `max` is not
/// positive or `value` is not positive: a chart never draws a negative or undefined length.
pub fn scale(value: f64, max: f64) -> i64 {
    if max <= 0.0 || value <= 0.0 {
        return 0;
    }
    (value / max * FULL as f64).round() as i64
}

/// A tenths value as a fixed one-decimal coordinate: `123` -> `"12.3"`, `120` -> `"12"`,
/// `-5` -> `"-0.5"`. A whole value drops its `.0`, so integer coordinates stay short.
pub fn tenths(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let magnitude = value.unsigned_abs();
    let whole = magnitude / 10;
    match magnitude % 10 {
        0 => format!("{sign}{whole}"),
        frac => format!("{sign}{whole}.{frac}"),
    }
}

#[cfg(test)]
mod tests;
