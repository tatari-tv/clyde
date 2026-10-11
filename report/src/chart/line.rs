//! The by-day line chart: a polyline over the zero-filled [`DayRow`] series.
//!
//! Rust computes every coordinate; no model ever sees or writes chart geometry (render inversion,
//! `docs/design/2026-07-29-render-inversion.md`). The output is two kinds of value:
//!
//! - geometry: [`LineChart::viewbox`], [`LineChart::points`] and [`LineChart::coords`], whole user
//!   units in a fixed [`VIEWBOX`], which a backend writes through its numeric geometry path;
//! - labels: [`LineChart::y_labels`] / [`LineChart::x_labels`], display strings formatted by
//!   [`crate::fmt`], which a backend writes as text.
//!
//! The markdown backend draws this as a sibling SVG asset (or a table under `eval`); the HTML
//! backend draws it inline with per-day hover columns (`render/html/line.rs`).

use crate::aggregate::DayRow;
use crate::fmt::{format_int, format_usd};
use log::{debug, trace};
use serde::Serialize;

/// The `viewBox`, identical for every line chart: the series is scaled into it, so one fixed box
/// keeps every chart's coordinates comparable.
pub const VIEWBOX: &str = "0 0 1000 300";

/// `viewBox` width, in user units. Kept in sync with [`VIEWBOX`] by
/// [`tests::viewbox_const_matches_the_plot_dimensions`].
const PLOT_WIDTH: f64 = 1000.0;

/// `viewBox` height, in user units.
const PLOT_HEIGHT: f64 = 300.0;

/// Vertical breathing room at the top and bottom of the plot, in user units: the series max plots
/// at `PLOT_MARGIN` rather than at `0`, so a stroke drawn on the maximum is not clipped in half by
/// the viewBox edge.
const PLOT_MARGIN: f64 = 10.0;

/// Most x-axis labels a chart carries. A 200-day window has one point per day and cannot legibly
/// carry 200 date labels, so the dates are subsampled to this many, first and last always included.
const MAX_X_LABELS: usize = 6;

/// Fewest rows a line chart is drawn from. One point is not a line; a single-row series renders as
/// a table instead (the same "absent field -> not a chart" rule `*-percent-of-max` already uses).
const MIN_POINTS: usize = 2;

/// One precomputed line chart: its geometry plus the display labels around it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct LineChart {
    /// The `viewBox` attribute value, verbatim. Always [`VIEWBOX`].
    pub viewbox: String,
    /// The `points` attribute value, verbatim: `"0,287 34,120 68,44 ..."`, one point per row of the
    /// series, in row order, written into `points="..."` as-is.
    pub points: String,
    /// Y-axis display strings, top to bottom: the series max, its midpoint, and zero.
    pub y_labels: Vec<String>,
    /// X-axis display strings: the series' dates, subsampled to at most [`MAX_X_LABELS`], first and
    /// last always present.
    pub x_labels: Vec<String>,
    /// The same points as [`Self::points`], as integer `(x, y)` pairs in row order, for a backend
    /// that draws per-point geometry (hover columns). Never serialized: [`Self::points`] is the
    /// one string the context block carries.
    #[serde(skip)]
    pub coords: Vec<(i64, i64)>,
}

/// The y coordinate of the three horizontal gridlines, top to bottom: the series max, its midpoint,
/// and zero. They line up with [`LineChart::y_labels`] one for one.
pub const GRIDLINE_YS: [i64; 3] = [
    PLOT_MARGIN as i64,
    (PLOT_HEIGHT / 2.0) as i64,
    (PLOT_HEIGHT - PLOT_MARGIN) as i64,
];

/// The zero line's y coordinate, where an area fill under a line closes.
pub const BASELINE_Y: i64 = (PLOT_HEIGHT - PLOT_MARGIN) as i64;

/// The `viewBox` width as an integer coordinate, where an area fill's closing edge sits.
pub const PLOT_RIGHT: i64 = PLOT_WIDTH as i64;

/// The window's precomputed charts. A field is ABSENT (never an empty or flat chart) when its
/// series cannot honestly be drawn as a line: fewer than [`MIN_POINTS`] rows, or an all-zero
/// series with no scale. A backend draws an absent chart as a table (markdown) or an empty state
/// (HTML).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Charts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_day_spend: Option<LineChart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_day_sessions: Option<LineChart>,
}

/// Build both by-day charts from the already-zero-filled [`DayRow`] series (Phase 4): one row per
/// calendar day means the polyline's x axis is the calendar, and a gap is a visible dip rather than
/// a missing point.
pub fn compute_charts(by_day: &[DayRow]) -> Charts {
    debug!("chart::compute_charts: rows={}", by_day.len());
    let dates: Vec<&str> = by_day.iter().map(|r| r.date.as_str()).collect();
    let spend: Vec<f64> = by_day.iter().map(|r| r.spend_raw).collect();
    let sessions: Vec<f64> = by_day.iter().map(|r| r.sessions as f64).collect();

    let charts = Charts {
        by_day_spend: line_chart(&spend, &dates, format_usd),
        by_day_sessions: line_chart(&sessions, &dates, |v| format_int(v.round() as u64)),
    };
    debug!(
        "chart::compute_charts: spend-chart={} sessions-chart={} points-bytes={}",
        charts.by_day_spend.is_some(),
        charts.by_day_sessions.is_some(),
        charts.by_day_spend.as_ref().map_or(0, |c| c.points.len()),
    );
    charts
}

/// One chart over `values`, labeled with `dates` on x and `label`-formatted magnitudes on y.
/// `None` when the series is too short or has no positive maximum to scale against.
fn line_chart(values: &[f64], dates: &[&str], label: impl Fn(f64) -> String) -> Option<LineChart> {
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    debug!(
        "chart::line_chart: values={} dates={} max={max}",
        values.len(),
        dates.len()
    );
    if values.len() < MIN_POINTS || values.len() != dates.len() {
        debug!("chart::line_chart: absent, series is shorter than {MIN_POINTS} rows or misaligned with its dates");
        return None;
    }
    if max <= 0.0 {
        debug!("chart::line_chart: absent, series maximum is not positive");
        return None;
    }
    let coords = coords(values, max);
    let chart = LineChart {
        viewbox: VIEWBOX.to_string(),
        points: points(&coords),
        y_labels: vec![label(max), label(max / 2.0), label(0.0)],
        x_labels: x_labels(dates),
        coords,
    };
    debug!(
        "chart::line_chart: points-bytes={} y-labels={:?} x-labels={}",
        chart.points.len(),
        chart.y_labels,
        chart.x_labels.len()
    );
    Some(chart)
}

/// Each value's `(x, y)` in whole user units. x spreads the series evenly across the full
/// [`PLOT_WIDTH`]; y maps the series max to [`PLOT_MARGIN`] and zero to `PLOT_HEIGHT - PLOT_MARGIN`.
///
/// Callers guarantee at least [`MIN_POINTS`] values and a positive `max`.
fn coords(values: &[f64], max: f64) -> Vec<(i64, i64)> {
    let last = (values.len() - 1) as f64;
    let plot = PLOT_HEIGHT - 2.0 * PLOT_MARGIN;
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let x = (i as f64 / last * PLOT_WIDTH).round() as i64;
            let y = (PLOT_HEIGHT - PLOT_MARGIN - (value / max) * plot).round() as i64;
            trace!("chart::coords: i={i} value={value} x={x} y={y}");
            (x, y)
        })
        .collect()
}

/// The `points` attribute value: `x,y` pairs space-separated, one per value, rounded to whole user
/// units so the string stays short on a long window.
fn points(coords: &[(i64, i64)]) -> String {
    let mut out = String::new();
    for (i, (x, y)) in coords.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{x},{y}"));
    }
    out
}

/// The x-axis dates, subsampled to at most [`MAX_X_LABELS`] evenly spaced entries with the first
/// and last always included. A long window (`--since 2026-01-01` is 200+ rows) keeps every POINT
/// and loses only labels: the line stays honest, the axis stays legible.
fn x_labels(dates: &[&str]) -> Vec<String> {
    if dates.len() <= MAX_X_LABELS {
        return dates.iter().map(|d| (*d).to_string()).collect();
    }
    let last = (dates.len() - 1) as f64;
    let steps = (MAX_X_LABELS - 1) as f64;
    let labels: Vec<String> = (0..MAX_X_LABELS)
        .filter_map(|i| {
            let index = (i as f64 / steps * last).round() as usize;
            trace!("chart::x_labels: label={i} index={index}");
            dates.get(index).map(|d| (*d).to_string())
        })
        .collect();
    debug!(
        "chart::x_labels: dates={} subsampled to labels={}",
        dates.len(),
        labels.len()
    );
    labels
}

#[cfg(test)]
mod tests;
