#![allow(clippy::unwrap_used)]

//! Phase 11 success criteria, against a REAL context block -- `build_context_block` output and the
//! `QuotableFacts` built beside it, not a hand-written fact set -- so what is proven here is what
//! the renderer actually enforces on a live artifact.

use super::*;
use crate::chart::VIEWBOX;
use crate::geometry::reject_foreign_geometry;
use crate::quotable::RenderContext;

/// A window with one session per day over `days` days, spend ramping so the series has a real
/// shape and a real maximum.
fn windowed_report(days: usize) -> Report {
    let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let mut sessions = BTreeMap::new();
    for i in 0..days {
        let date = start + chrono::Duration::days(i as i64);
        let mut models = BTreeMap::new();
        models.insert("claude-opus-4-7".into(), opus_tokens());
        sessions.insert(
            format!(
                "{:08x}-1111-4222-8333-444444444444",
                0xa14bc3d2u32.wrapping_add(i as u32)
            ),
            session_entry(
                Some("ship the thing"),
                Some("tatari-tv/clyde"),
                ts(&format!("{}T09:14:22Z", date.format("%Y-%m-%d"))),
                ts(&format!("{}T11:02:41Z", date.format("%Y-%m-%d"))),
                Some(1.0 + i as f64),
                models,
                None,
            ),
        );
    }
    let mut totals_models = BTreeMap::new();
    totals_models.insert("claude-opus-4-7".into(), opus_tokens());
    let until = start + chrono::Duration::days(days as i64 - 1);
    Report {
        schema_version: 2,
        generated: ts("2026-08-01T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-01-01T00:00:00Z"),
        until: ts(&format!("{}T23:59:59Z", until.format("%Y-%m-%d"))),
        outcomes_enabled: Some(true),
        notes: Vec::new(),
        totals: totals(days, (0..days).map(|i| 1.0 + i as f64).sum(), totals_models),
        sessions,
    }
}

fn context(report: &Report) -> RenderContext {
    build_context_block(
        report,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        None,
        None,
    )
    .unwrap()
}

/// The chart the binary computed for a window, straight off the aggregates -- the same values the
/// context block carries, so a test can assert the artifact copied them rather than restating them.
fn spend_chart(report: &Report) -> crate::chart::LineChart {
    crate::aggregate::compute(report, 0, &pricing())
        .charts
        .by_day_spend
        .unwrap()
}

/// The authorized artifact: the two verbatim strings, in the one `<svg>`/`<polyline>` shape the
/// prompt licenses, with the axis labels as HTML outside the chart.
fn artifact(chart: &crate::chart::LineChart) -> String {
    format!(
        "<!doctype html><html><body><section class=\"chart\">\
         <ul class=\"y-axis\"><li>{}</li><li>{}</li><li>{}</li></ul>\
         <svg viewBox=\"{}\" class=\"spark\"><title>Spend by day</title>\
         <polyline points=\"{}\" fill=\"none\" stroke=\"currentColor\"/></svg>\
         <ul class=\"x-axis\"><li>{}</li></ul></section></body></html>",
        chart.y_labels[0],
        chart.y_labels[1],
        chart.y_labels[2],
        chart.viewbox,
        chart.points,
        chart.x_labels.join("</li><li>"),
    )
}

/// Success criterion 1: the rendered artifact carries a `<polyline>` whose `points` string matches
/// the context BYTE FOR BYTE. Asserted from both ends -- the string is in the context JSON, and the
/// artifact's attribute is that exact string -- so neither side can drift alone.
///
/// BITES: change one coordinate in the artifact and the byte-for-byte assertion fails; change one
/// in `chart::points` and the context-JSON assertion fails.
#[test]
fn the_artifact_polyline_matches_the_context_byte_for_byte() {
    let report = windowed_report(30);
    let ctx = context(&report);
    let chart = spend_chart(&report);
    let html = artifact(&chart);

    assert!(
        ctx.json.contains(&format!("\"points\":\"{}\"", chart.points)),
        "the context block must carry the points string the artifact copies"
    );
    assert!(
        ctx.json.contains(&format!("\"viewbox\":\"{VIEWBOX}\"")),
        "the context block must carry the binary-owned viewBox"
    );
    let attr = format!("points=\"{}\"", chart.points);
    assert!(
        html.contains(&attr),
        "the artifact must carry the points string verbatim"
    );
    reject_foreign_geometry("html", &html, &ctx.facts).unwrap();
    reject_foreign_numbers("html", &visible_text(&html), &ctx.facts).unwrap();
}

/// Success criterion 2, all three fabrications, each planted into an OTHERWISE VALID artifact and
/// each failing on its own: a fabricated `points` list, a `<path d>`, and a `<circle cx cy>`.
///
/// BITES: comment out the `facts.licenses_geometry` check and case 1 passes; drop `path`/`circle`
/// from the element check (i.e. permit everything) and cases 2 and 3 pass.
#[test]
fn each_planted_fabrication_independently_fails_the_render() {
    let report = windowed_report(30);
    let ctx = context(&report);
    let chart = spend_chart(&report);
    let clean = artifact(&chart);
    reject_foreign_geometry("html", &clean, &ctx.facts).unwrap();

    // 1: a fabricated points list -- plausible, well-formed, and not what the binary computed.
    let fabricated = clean.replace(&chart.points, "0,280 120,140 240,60 360,20 480,90 600,30");
    let err = reject_foreign_geometry("html", &fabricated, &ctx.facts).unwrap_err();
    assert!(
        format!("{err}").contains("not one the binary computed"),
        "a fabricated points list must fail: {err}"
    );

    // 2: a planted <path d="...">, the element the prompt has always banned. Asserted against the
    // ELEMENT rule's wording, not just the element name: `d` is also an unpermitted attribute, and
    // a test that accepted either message would stay green with the element allowlist deleted.
    let with_path = clean.replace("</svg>", r#"<path d="M0,280 C120,140 240,60 360,20"/></svg>"#);
    let err = reject_foreign_geometry("html", &with_path, &ctx.facts).unwrap_err();
    assert!(
        format!("{err}").contains("put a <path> inside an <svg> chart subtree"),
        "a planted <path> must fail on the element allowlist: {err}"
    );

    // 3: a planted <circle cx cy>, the model computing a point marker for itself.
    let with_circle = clean.replace("</svg>", r#"<circle cx="500" cy="150" r="4"/></svg>"#);
    let err = reject_foreign_geometry("html", &with_circle, &ctx.facts).unwrap_err();
    assert!(
        format!("{err}").contains("put a <circle> inside an <svg> chart subtree"),
        "a planted <circle> must fail on the element allowlist: {err}"
    );
}

/// Success criterion 3: an element outside the permitted set, inside a chart subtree, fails --
/// including one that carries no numbers at all. The allowlist is over ELEMENTS, not over the
/// numbers they happen to hold.
#[test]
fn an_element_outside_the_permitted_set_fails_inside_a_chart_subtree() {
    let report = windowed_report(30);
    let ctx = context(&report);
    let chart = spend_chart(&report);
    let clean = artifact(&chart);

    for planted in [
        r#"<rect width="1000" height="300" fill="none"/>"#,
        "<desc>spend by day</desc>",
        r#"<image href="chart.png"/>"#,
    ] {
        let html = clean.replace("</svg>", &format!("{planted}</svg>"));
        let err = reject_foreign_geometry("html", &html, &ctx.facts).unwrap_err();
        assert!(
            format!("{err}").contains("inside an <svg> chart subtree"),
            "planted {planted} must fail: {err}"
        );
    }
}

/// Phase 4's long-window watch-out, carried to its end: `--since 2026-01-01` is 210 by-day rows and
/// a 210-point polyline. It renders, it validates, and the geometry it adds to the context block is
/// negligible against the render ceilings (`html-max-output-tokens`, 32,000 by default).
#[test]
fn a_two_hundred_day_window_renders_and_validates() {
    let report = windowed_report(210);
    let ctx = context(&report);
    let chart = spend_chart(&report);
    let html = artifact(&chart);

    assert_eq!(chart.points.split(' ').count(), 210);
    println!(
        "210-day window: context_bytes={} points_bytes={} artifact_bytes={}",
        ctx.json.len(),
        chart.points.len(),
        html.len()
    );
    assert!(
        chart.points.len() < 2_500,
        "a 210-row polyline is {} bytes",
        chart.points.len()
    );
    reject_foreign_geometry("html", &html, &ctx.facts).unwrap();
    reject_foreign_numbers("html", &visible_text(&html), &ctx.facts).unwrap();
}

/// The same geometry measured against a REAL collected window instead of a fixture:
/// `CLYDE_REAL_REPORT=/path/to/claude-report.json cargo test -p report -- --ignored measure_chart`.
/// Ignored by default (CI has no collected artifact), mirroring the Phase 10 measurement test.
#[test]
#[ignore = "needs a real `report collect` artifact, path in CLYDE_REAL_REPORT"]
fn measure_chart_geometry_on_a_real_window() {
    let Ok(path) = std::env::var("CLYDE_REAL_REPORT") else {
        panic!("set CLYDE_REAL_REPORT to a `report collect` artifact");
    };
    let body = std::fs::read_to_string(&path).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
    let ctx = context(&report);
    let charts = crate::aggregate::compute(&report, 0, &pricing()).charts;
    let spend = charts.by_day_spend.unwrap();

    println!(
        "real window: sessions={} context_bytes={} points={} points_bytes={} x-labels={:?} y-labels={:?}",
        report.sessions.len(),
        ctx.json.len(),
        spend.points.split(' ').count(),
        spend.points.len(),
        spend.x_labels,
        spend.y_labels,
    );
    reject_foreign_geometry("html", &artifact(&spend), &ctx.facts).unwrap();
}

/// Phase 11's prompt-edit ledger. The unlock is a NARROWING of one ban, not a loosening: the html
/// template keeps the model-authored-coordinate sentence VERBATIM and adds a scoped exception for
/// the two copied strings, spells out both allowlists, and the markdown template is told to ignore
/// the charts entirely (it cannot draw SVG, and a copied coordinate would fail the prose guard).
///
/// BITES: delete the "You MUST NOT emit SVG coordinate geometry of any kind" sentence, or drop
/// either allowlist from the prompt, and the matching assertion fails.
#[test]
fn the_html_template_authorizes_only_the_two_verbatim_strings() {
    assert!(
        DEFAULT_HTML_PROMPT.contains(
            "You MUST NOT emit SVG coordinate geometry of any kind: no `viewBox` math, no `<path>`/`<polyline>`\n\
             point lists, no x/y positions, no axis ticks, no gridline offsets, no radii, no angles."
        ),
        "the ban on model-authored coordinates must stay VERBATIM"
    );
    assert!(
        DEFAULT_HTML_PROMPT.contains("aggregates.charts.by-day-spend")
            && DEFAULT_HTML_PROMPT.contains("aggregates.charts.by-day-sessions"),
        "the html template must name both authorized fields"
    );
    assert!(
        DEFAULT_HTML_PROMPT.contains("BYTE FOR BYTE"),
        "the html template must state that the two strings are copied byte for byte"
    );
    for permitted in [
        "`svg`, `polyline`, `g`, `text`, `title`",
        "`viewBox`, `points`, `class`",
    ] {
        assert!(
            DEFAULT_HTML_PROMPT.contains(permitted),
            "the html template must spell out the allowlist the render enforces: {permitted}"
        );
    }
    assert!(
        DEFAULT_HTML_PROMPT.contains("NO attribute inside the `<svg>` may contain a digit"),
        "the html template must state the digit rule, which is what fails an unanticipated attribute"
    );
    assert!(
        DEFAULT_PROMPT.contains("`aggregates.charts`: SVG line-chart geometry for the HTML report. IGNORE IT"),
        "the markdown template must be told the charts are not its data"
    );
}

/// The charts reach the model at the documented path, and their labels are quotable prose figures
/// while their coordinates are not. This is the Phase 10 separation holding under Phase 11's load.
#[test]
fn charts_land_under_aggregates_and_only_their_labels_are_quotable() {
    let report = windowed_report(30);
    let ctx = context(&report);
    let chart = spend_chart(&report);

    assert!(ctx.json.contains("\"charts\":{\"by-day-spend\":"));
    assert!(ctx.json.contains("\"by-day-sessions\":"));
    // A y label is a display string the prose may state.
    assert_eq!(ctx.facts.foreign_figures(&chart.y_labels[0]), Vec::<String>::new());
    // The polyline's coordinates are not. `290` is the baseline y of every zero-spend day and
    // appears in no display string, so stating it in prose is a fabrication.
    assert_eq!(
        ctx.facts.foreign_figures("the chart bottoms out at 290"),
        vec!["290".to_string()],
        "a polyline coordinate must never become a quotable prose figure"
    );
}
