//! The by-day line charts as inline SVG: daily active sessions, and the daily cache-read share.
//!
//! Geometry comes from [`crate::chart`]: the points, the gridline heights and the baseline. This
//! module places them and adds per-day hover columns, whose edges are the midpoints between
//! neighbouring points. Axis labels are HTML beside the SVG rather than SVG text, so they stay
//! legible at any width while the plot stretches (`preserveAspectRatio="none"`).

use super::page::Page;
use crate::aggregate::DayRow;
use crate::chart::{self, LineChart};
use crate::render::sections;
use crate::render::visuals::ShareChart;

/// Shown in place of a chart whose series cannot honestly be drawn as a line.
const EMPTY_CHART: &str = "Not enough daily activity in this window to draw a line.";

/// The captioned daily-sessions chart. `chart` is `None` when the series is too short or all zero;
/// the figure still renders, with an empty state instead of a plot.
pub(super) fn sessions_figure(p: &mut Page, label: &str, chart: Option<&LineChart>, rows: &[DayRow]) {
    p.raw("<figure class=\"chart tone-sessions\">");
    p.raw("<figcaption>").text(label).raw("</figcaption>");
    match chart {
        Some(chart) => plot(p, label, chart, rows),
        None => {
            p.raw("<p class=\"chart-empty\">").text(EMPTY_CHART).raw("</p>");
        }
    }
    p.raw("</figure>\n");
}

/// The y-axis labels beside a plot, top, middle, bottom.
pub(super) fn y_axis(p: &mut Page, labels: &[String]) {
    p.raw("<div class=\"y-axis\" aria-hidden=\"true\">");
    for (index, text) in labels.iter().enumerate() {
        p.raw(match index {
            0 => "<span class=\"y-top\">",
            1 => "<span class=\"y-mid\">",
            _ => "<span class=\"y-low\">",
        });
        p.text(text).raw("</span>");
    }
    p.raw("</div>");
}

/// The x-axis date labels under a plot.
pub(super) fn x_axis(p: &mut Page, labels: &[String]) {
    p.raw("<div class=\"x-axis\" aria-hidden=\"true\">");
    for text in labels {
        p.raw("<span>").text(text).raw("</span>");
    }
    p.raw("</div>");
}

/// The stretched SVG and its three gridlines, left open for the series.
fn open_svg(p: &mut Page, viewbox: &str, label: &str) {
    p.raw("<svg").geom("viewBox", viewbox);
    p.raw(" preserveAspectRatio=\"none\" role=\"img\"")
        .attr("aria-label", label)
        .raw(">");
    p.raw("<g class=\"grid\">");
    for y in chart::GRIDLINE_YS {
        p.raw("<line x1=\"0\"").geom_int("x2", chart::PLOT_RIGHT);
        p.geom_int("y1", y).geom_int("y2", y).raw("/>");
    }
    p.raw("</g>");
}

fn plot(p: &mut Page, label: &str, chart: &LineChart, rows: &[DayRow]) {
    p.raw("<div class=\"plot\">");
    y_axis(p, &chart.y_labels);
    open_svg(p, &chart.viewbox, label);
    let area = format!(
        "0,{base} {} {right},{base}",
        chart.points,
        base = chart::BASELINE_Y,
        right = chart::PLOT_RIGHT
    );
    p.raw("<polygon class=\"area\"").geom("points", &area).raw("/>");
    p.raw("<polyline class=\"line\"")
        .geom("points", &chart.points)
        .raw("/>");
    let tips: Vec<String> = rows
        .iter()
        .map(|row| format!("{}: {}", row.date, sections::plural(row.sessions, "session")))
        .collect();
    let xs: Vec<i64> = chart.coords.iter().map(|(x, _)| *x).collect();
    hover_columns(p, &xs, &tips);
    p.raw("</svg>");
    x_axis(p, &chart.x_labels);
    p.raw("</div>");
}

/// The captioned daily cache-read share: one polyline per run of days that had tokens, on a fixed
/// 0-100% scale, so a day with no tokens is a gap rather than a fall to zero.
pub(super) fn share_figure(p: &mut Page, label: &str, chart: Option<&ShareChart>) {
    p.raw("<figure class=\"chart tone-cache\">");
    p.raw("<figcaption>").text(label).raw("</figcaption>");
    let Some(chart) = chart else {
        p.raw("<p class=\"chart-empty\">")
            .text(EMPTY_CHART)
            .raw("</p></figure>\n");
        return;
    };
    p.raw("<div class=\"plot\">");
    y_axis(p, &chart.y_labels);
    open_svg(p, &chart.line.viewbox, label);
    for run in &chart.line.runs {
        p.raw("<polyline class=\"line\"").geom("points", run).raw("/>");
    }
    hover_columns(p, &chart.line.xs, &chart.tips);
    p.raw("</svg>");
    x_axis(p, &chart.line.x_labels);
    p.raw("</div></figure>\n");
}

/// One transparent full-height column per day, carrying that day's figure as a tooltip. Without
/// JS the `<title>` child is the browser's native tooltip; with JS, `app.js` reads `data-tip`.
fn hover_columns(p: &mut Page, xs: &[i64], tips: &[String]) {
    if tips.len() != xs.len() {
        log::warn!(
            "html::line::hover_columns: {} rows against {} points; drawing no hover columns",
            tips.len(),
            xs.len()
        );
        return;
    }
    p.raw("<g class=\"hits\">");
    for (left, right, tip) in columns(xs, chart::PLOT_RIGHT)
        .into_iter()
        .zip(tips)
        .map(|((l, r), t)| (l, r, t))
    {
        p.raw("<rect").geom_int("x", left).raw(" y=\"0\"");
        p.geom_int("width", right - left).raw(" height=\"100%\"");
        p.attr("data-tip", tip).raw("><title>").text(tip).raw("</title></rect>");
    }
    p.raw("</g>");
}

/// `(left, right)` x edges of each point's hover column: halfway to each neighbour, and the plot
/// edge for the first and last point.
pub(super) fn columns(xs: &[i64], right_edge: i64) -> Vec<(i64, i64)> {
    xs.iter()
        .enumerate()
        .map(|(i, x)| {
            let left = if i == 0 { 0 } else { (xs[i - 1] + x) / 2 };
            let right = xs.get(i + 1).map_or(right_edge, |next| (x + next) / 2);
            (left, right)
        })
        .collect()
}
