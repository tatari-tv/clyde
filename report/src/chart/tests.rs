#![allow(clippy::unwrap_used)]

//! Phase 11: the precomputed line-chart geometry. Everything here is pure arithmetic over a
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
            spend_percent_of_max: None,
            sessions_percent_of_max: None,
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
                spend_percent_of_max: None,
                sessions_percent_of_max: None,
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
