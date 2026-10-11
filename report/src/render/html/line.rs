//! The by-day line chart as inline SVG.
//!
//! Geometry comes from [`crate::chart`]: the points, the gridline heights and the baseline. This
//! module places them and adds per-day hover columns, whose edges are the midpoints between
//! neighbouring points. Axis labels are HTML beside the SVG rather than SVG text, so they stay
//! legible at any width while the plot stretches (`preserveAspectRatio="none"`).

use super::page::Page;
use crate::aggregate::DayRow;
use crate::chart::{self, LineChart};
use crate::render::sections;

/// Which accent a chart draws in.
#[derive(Debug, Clone, Copy)]
pub(super) enum Tone {
    Spend,
    Sessions,
}

/// Shown in place of a chart whose series cannot honestly be drawn as a line.
const EMPTY_CHART: &str = "Not enough daily activity in this window to draw a line.";

/// One captioned by-day chart. `chart` is `None` when the series is too short or all zero; the
/// figure still renders, with an empty state instead of a plot.
pub(super) fn figure(p: &mut Page, label: &str, chart: Option<&LineChart>, rows: &[DayRow], tone: Tone) {
    p.raw(match tone {
        Tone::Spend => "<figure class=\"chart tone-spend\">",
        Tone::Sessions => "<figure class=\"chart tone-sessions\">",
    });
    p.raw("<figcaption>").text(label).raw("</figcaption>");
    match chart {
        Some(chart) => plot(p, label, chart, rows, tone),
        None => {
            p.raw("<p class=\"chart-empty\">").text(EMPTY_CHART).raw("</p>");
        }
    }
    p.raw("</figure>\n");
}

fn plot(p: &mut Page, label: &str, chart: &LineChart, rows: &[DayRow], tone: Tone) {
    p.raw("<div class=\"plot\"><div class=\"y-axis\" aria-hidden=\"true\">");
    for (index, text) in chart.y_labels.iter().enumerate() {
        p.raw(match index {
            0 => "<span class=\"y-top\">",
            1 => "<span class=\"y-mid\">",
            _ => "<span class=\"y-low\">",
        });
        p.text(text).raw("</span>");
    }
    p.raw("</div>");

    p.raw("<svg").geom("viewBox", &chart.viewbox);
    p.raw(" preserveAspectRatio=\"none\" role=\"img\"")
        .attr("aria-label", label)
        .raw(">");
    p.raw("<g class=\"grid\">");
    for y in chart::GRIDLINE_YS {
        p.raw("<line x1=\"0\"").geom_int("x2", chart::PLOT_RIGHT);
        p.geom_int("y1", y).geom_int("y2", y).raw("/>");
    }
    p.raw("</g>");
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
    hover_columns(p, chart, rows, tone);
    p.raw("</svg>");

    p.raw("<div class=\"x-axis\" aria-hidden=\"true\">");
    for text in &chart.x_labels {
        p.raw("<span>").text(text).raw("</span>");
    }
    p.raw("</div></div>");
}

/// One transparent full-height column per day, carrying that day's figure as a tooltip. Without
/// JS the `<title>` child is the browser's native tooltip; with JS, `app.js` reads `data-tip`.
fn hover_columns(p: &mut Page, chart: &LineChart, rows: &[DayRow], tone: Tone) {
    let tips = tips(rows, tone);
    if tips.len() != chart.coords.len() {
        log::warn!(
            "html::line::hover_columns: {} rows against {} points; drawing no hover columns",
            tips.len(),
            chart.coords.len()
        );
        return;
    }
    p.raw("<g class=\"hits\">");
    for (left, right, tip) in columns(&chart.coords, chart::PLOT_RIGHT)
        .into_iter()
        .zip(tips)
        .map(|((l, r), t)| (l, r, t))
    {
        p.raw("<rect").geom_int("x", left).raw(" y=\"0\"");
        p.geom_int("width", right - left).raw(" height=\"100%\"");
        p.attr("data-tip", &tip)
            .raw("><title>")
            .text(&tip)
            .raw("</title></rect>");
    }
    p.raw("</g>");
}

/// The tooltip text for each day: `date: figure`.
fn tips(rows: &[DayRow], tone: Tone) -> Vec<String> {
    match tone {
        Tone::Spend => sections::day_rows(rows, true)
            .into_iter()
            .map(|(date, value)| format!("{date}: {value}"))
            .collect(),
        Tone::Sessions => rows
            .iter()
            .map(|row| format!("{}: {}", row.date, sections::plural(row.sessions, "session")))
            .collect(),
    }
}

/// `(left, right)` x edges of each point's hover column: halfway to each neighbour, and the plot
/// edge for the first and last point.
pub(super) fn columns(coords: &[(i64, i64)], right_edge: i64) -> Vec<(i64, i64)> {
    coords
        .iter()
        .enumerate()
        .map(|(i, (x, _))| {
            let left = if i == 0 { 0 } else { (coords[i - 1].0 + x) / 2 };
            let right = coords.get(i + 1).map_or(right_edge, |(next, _)| (x + next) / 2);
            (left, right)
        })
        .collect()
}
