#![allow(clippy::unwrap_used)]

//! The by-day line-chart geometry. Everything here is pure arithmetic over a
//! [`DayRow`] series, so the assertions are exact strings -- a coordinate that drifts is a chart
//! that lies about the data behind it.

use super::*;

/// A by-day series with the given (date, sessions, spend) shape.
fn rows(series: &[(&str, usize, f64)]) -> Vec<DayRow> {
    series
        .iter()
        .map(|(date, sessions, spend)| DayRow {
            date: (*date).to_string(),
            sessions: *sessions,
            spend_raw: *spend,
            spend: format_usd(*spend),
            active: *sessions > 0,
            ..DayRow::default()
        })
        .collect()
}

/// A `days`-long series whose spend ramps by day, for the long-window checks.
fn long_series(days: usize) -> Vec<DayRow> {
    (0..days)
        .map(|i| {
            let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i as i64);
            let spend = 1.0 + i as f64;
            DayRow {
                date: date.format("%Y-%m-%d").to_string(),
                sessions: i % 7,
                spend_raw: spend,
                spend: format_usd(spend),
                active: i % 7 > 0,
                ..DayRow::default()
            }
        })
        .collect()
}

/// The viewBox const and the plot dimensions are two spellings of one fact, and a chart drawn
/// against a viewBox that does not match its coordinates is silently clipped.
///
/// BITES: change `PLOT_WIDTH` to 900.0 and this fails.
#[test]
fn viewbox_const_matches_the_plot_dimensions() {
    assert_eq!(VIEWBOX, format!("0 0 {} {}", PLOT_WIDTH as i64, PLOT_HEIGHT as i64));
}

/// The exact geometry for a three-day series: x spread across the full width, the max at the top
/// margin, zero at the bottom margin, the midpoint halfway between.
///
/// BITES: drop the margin from the y mapping and every coordinate here moves.
#[test]
fn points_map_the_series_onto_the_viewbox() {
    let by_day = rows(&[
        ("2026-04-01", 1, 0.0),
        ("2026-04-02", 2, 50.0),
        ("2026-04-03", 3, 100.0),
    ]);
    let chart = compute_charts(&by_day).by_day_spend.unwrap();

    assert_eq!(chart.viewbox, VIEWBOX);
    assert_eq!(chart.points, "0,290 500,150 1000,10");
}

/// Y labels are the series max, its midpoint and zero, formatted for their series: dollars for
/// spend, comma-grouped counts for sessions.
#[test]
fn y_labels_are_display_strings_at_max_mid_and_zero() {
    let by_day = rows(&[("2026-04-01", 0, 0.0), ("2026-04-02", 41, 104.94)]);
    let charts = compute_charts(&by_day);

    assert_eq!(
        charts.by_day_spend.unwrap().y_labels,
        vec!["$104.94", "$52.47", "$0.00"]
    );
    assert_eq!(charts.by_day_sessions.unwrap().y_labels, vec!["41", "21", "0"]);
}

/// A short series keeps every date as an x label.
#[test]
fn x_labels_keep_every_date_on_a_short_series() {
    let by_day = rows(&[("2026-04-01", 1, 1.0), ("2026-04-02", 2, 2.0), ("2026-04-03", 3, 3.0)]);
    let chart = compute_charts(&by_day).by_day_spend.unwrap();

    assert_eq!(chart.x_labels, vec!["2026-04-01", "2026-04-02", "2026-04-03"]);
}

/// Phase 4's watch-out, now Phase 11's: `--since 2026-01-01` emits 200+ by-day rows. Every row
/// keeps its POINT (the line stays honest); only the labels subsample, first and last included.
/// The resulting `points` string is small enough to be irrelevant against the render ceilings.
///
/// BITES: subsample the points instead of the labels and the point count assertion fails.
#[test]
fn a_two_hundred_day_window_keeps_every_point_and_subsamples_only_labels() {
    let by_day = long_series(210);
    let chart = compute_charts(&by_day).by_day_spend.unwrap();

    assert_eq!(chart.points.split(' ').count(), 210, "one point per by-day row");
    assert_eq!(chart.x_labels.len(), MAX_X_LABELS);
    assert_eq!(chart.x_labels.first().unwrap(), "2026-01-01");
    assert_eq!(chart.x_labels.last().unwrap(), "2026-07-29");
    // ~9 bytes per point. Stated as a bound rather than an exact length so the assertion is about
    // the render ceilings (32,000 output tokens) and not about coordinate rounding.
    assert!(
        chart.points.len() < 2_500,
        "a 210-row polyline is {} bytes, which must stay negligible against the output ceiling",
        chart.points.len()
    );
}

/// An all-zero series has no scale to draw against, so the chart is ABSENT rather than a flat line
/// implying a measured shape. Same rule `percent_of_max` already applies to bars.
#[test]
fn an_all_zero_series_has_no_chart() {
    let by_day = rows(&[("2026-04-01", 0, 0.0), ("2026-04-02", 0, 0.0)]);
    let charts = compute_charts(&by_day);

    assert!(charts.by_day_spend.is_none());
    assert!(charts.by_day_sessions.is_none());
}

/// One point is not a line: a single-day window renders as a table.
#[test]
fn a_single_row_series_has_no_chart() {
    let by_day = rows(&[("2026-04-01", 3, 12.50)]);
    let charts = compute_charts(&by_day);

    assert!(charts.by_day_spend.is_none());
    assert!(charts.by_day_sessions.is_none());
}

/// The two charts are computed independently: a window can have spend on every day but sessions
/// that only began on some of them, and each series is scaled against its OWN maximum.
#[test]
fn spend_and_session_charts_scale_against_their_own_maxima() {
    let by_day = rows(&[("2026-04-01", 4, 10.0), ("2026-04-02", 1, 40.0)]);
    let charts = compute_charts(&by_day);

    assert_eq!(charts.by_day_spend.unwrap().points, "0,220 1000,10");
    assert_eq!(charts.by_day_sessions.unwrap().points, "0,10 1000,220");
}

/// A max-1 integer series: the midpoint `0.5` prints as `1`, which repeats the top label, so it is
/// blanked rather than printed as `1, 1, 0`.
#[test]
fn a_midpoint_label_that_repeats_the_top_label_is_blanked() {
    let by_day = rows(&[("2026-04-01", 0, 0.0), ("2026-04-02", 1, 5.0)]);
    let chart = compute_charts(&by_day).by_day_sessions.unwrap();

    assert_eq!(chart.y_labels, vec!["1", "", "0"]);
}

/// The midpoint that repeats the bottom label is blanked too.
#[test]
fn a_midpoint_label_that_repeats_the_zero_label_is_blanked() {
    let labels = y_labels(0.008, &|v| format!("${v:.2}"));

    assert_eq!(labels, vec!["$0.01", "", "$0.00"]);
}

/// A midpoint that differs from both neighbors stays.
#[test]
fn a_distinct_midpoint_label_stays() {
    let labels = y_labels(4.0, &|v| format!("{v}"));

    assert_eq!(labels, vec!["4", "2", "0"]);
}

/// The share line breaks at a day with no value instead of joining across it or dropping to zero,
/// draws a lone valued day as a dash, and maps shares onto a fixed 0-100% scale.
#[test]
fn share_line_breaks_at_unvalued_days_on_a_fixed_scale() {
    let dates = ["d1", "d2", "d3", "d4", "d5"];
    let line = share_line(&[Some(1.0), Some(0.5), None, Some(0.0), None], &dates).unwrap();
    assert_eq!(line.viewbox, VIEWBOX);
    assert_eq!(line.xs, vec![0, 250, 500, 750, 1000]);
    assert_eq!(
        line.runs,
        vec!["0,10 250,150".to_string(), "746,290 754,290".to_string()]
    );
    assert_eq!(line.x_labels, dates.iter().map(|d| d.to_string()).collect::<Vec<_>>());
}

/// A lone point at an edge keeps its dash inside the plot, and an out-of-range share clamps.
#[test]
fn share_line_dashes_clamp_at_the_edges() {
    let line = share_line(&[Some(1.7), None, Some(-0.2)], &["a", "b", "c"]).unwrap();
    assert_eq!(line.runs, vec!["0,10 4,10".to_string(), "996,290 1000,290".to_string()]);
    let full = share_line(&[Some(0.25), Some(0.75)], &["a", "b"]).unwrap();
    assert_eq!(full.runs, vec!["0,220 1000,80".to_string()]);
}

#[test]
fn share_line_is_absent_without_a_drawable_series() {
    assert!(share_line(&[Some(0.5)], &["a"]).is_none(), "one day is not a line");
    assert!(share_line(&[None, None], &["a", "b"]).is_none(), "no valued day");
    assert!(share_line(&[Some(0.5), Some(0.5)], &["a"]).is_none(), "misaligned");
    assert!(share_line(&[], &[]).is_none());
}

/// `y_labels` is shared with the HTML backend's stacked daily chart: max, blanked duplicate mid, zero.
#[test]
fn y_labels_blank_a_midpoint_that_repeats_a_neighbour() {
    let money = |v: f64| format_usd(v);
    assert_eq!(y_labels(10.0, &money), vec!["$10.00", "$5.00", "$0.00"]);
    let int = |v: f64| format_int(v.round() as u64);
    assert_eq!(y_labels(1.0, &int), vec!["1", "", "0"]);
    assert_eq!(
        y_labels(0.0, &int),
        vec!["0", "", "0"],
        "a zero series blanks the mid against both"
    );
}
