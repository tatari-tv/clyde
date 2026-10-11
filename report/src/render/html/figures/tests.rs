#![allow(clippy::unwrap_used)]

//! Per-chart golden markup: each chart drawn from a small hand-built view, pinned byte for byte,
//! plus its zero-data empty state. A coordinate that drifts here is a chart that lies.

use super::*;
use crate::chart::{bars, calendar as cal, histogram as hist, stack as stk, treemap as tm, waterfall as wf};
use crate::render::visuals::{Column, PairRow, Ranks, StackSegment, Tile, TreeGroup};

fn draw(f: impl FnOnce(&mut Page)) -> String {
    let mut p = Page::new();
    f(&mut p);
    p.finish()
}

fn pairs_view() -> Pairs {
    let lengths = bars::paired(&[(0.6, 0.75), (0.4, 0.25)]).unwrap();
    let rows = [
        ("acme/api", "3 sessions", "60%", "$7.50", "75%"),
        ("acme/web", "2 sessions", "40%", "$2.50", "25%"),
    ];
    Pairs {
        rows: rows
            .iter()
            .zip(lengths)
            .enumerate()
            .map(|(i, ((label, sessions, ss, spend, sp), bars))| PairRow {
                label: (*label).to_string(),
                sessions_share: (*ss).to_string(),
                sessions: (*sessions).to_string(),
                spend_share: (*sp).to_string(),
                spend: (*spend).to_string(),
                per_session: "$2.50 per session".to_string(),
                tip: format!("{label}: tip"),
                bars,
                rank: Ranks {
                    spend: i,
                    sessions: i,
                    per_session: 1 - i,
                },
            })
            .collect(),
    }
}

fn treemap_view() -> Treemap {
    let laid = tm::treemap(&[(3.0, vec![2.0, 1.0]), (1.0, vec![])]).unwrap();
    let a = laid[0].clone().unwrap();
    let b = laid[1].clone().unwrap();
    Treemap {
        groups: vec![
            TreeGroup {
                label: "acme".to_string(),
                spend: "$3.00".to_string(),
                share: "75%".to_string(),
                tone: 0,
                rect: a.rect,
                tiles: vec![
                    Tile {
                        label: "api".to_string(),
                        spend: "$2.00".to_string(),
                        tip: "acme/api: $2.00 (50%)".to_string(),
                        rect: a.items[0].unwrap(),
                    },
                    Tile {
                        label: "web".to_string(),
                        spend: "$1.00".to_string(),
                        tip: "acme/web: $1.00 (25%)".to_string(),
                        rect: a.items[1].unwrap(),
                    },
                ],
            },
            TreeGroup {
                label: "(unattributed)".to_string(),
                spend: "$1.00".to_string(),
                share: "25%".to_string(),
                tone: 5,
                rect: b.rect,
                tiles: vec![Tile {
                    label: "(unattributed)".to_string(),
                    spend: "$1.00".to_string(),
                    tip: "(unattributed): $1.00 (25%)".to_string(),
                    rect: b.rect,
                }],
            },
        ],
    }
}

fn stack_view() -> Stack {
    let segs = stk::stack(&[3.0, 1.0]).unwrap();
    Stack {
        segments: vec![
            StackSegment {
                label: "opus".to_string(),
                share: "75%".to_string(),
                spend: "$3.00".to_string(),
                tip: "opus: $3.00 (75%)".to_string(),
                tone: 0,
                segment: segs[0],
            },
            StackSegment {
                label: "haiku".to_string(),
                share: "25%".to_string(),
                spend: "$1.00".to_string(),
                tip: "haiku: $1.00 (25%)".to_string(),
                tone: 1,
                segment: segs[1],
            },
        ],
    }
}

fn histogram_view() -> Histogram {
    let laid = hist::histogram(&[60, 70, 300]).unwrap();
    Histogram {
        columns: laid
            .counts
            .iter()
            .zip(&laid.heights)
            .enumerate()
            .map(|(i, (count, height))| Column {
                count: count.to_string(),
                tip: format!("bucket {i}"),
                height: *height,
                median: i == 0,
                p90: i == 2,
            })
            .collect(),
        edges: vec!["$0.50".into(), "$1".into(), "$2.50".into(), "$5".into()],
        unpriced: Some("1 session with no price is not shown.".to_string()),
    }
}

fn calendar_view() -> Calendar {
    let first = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let laid = cal::calendar(first, &[0.0, 4.0, 1.0]).unwrap();
    Calendar {
        height: laid.height,
        cells: laid
            .cells
            .into_iter()
            .zip(["2026-04-04: $0.00", "2026-04-05: $4.00", "2026-04-06: $1.00"])
            .map(|(c, t)| (c, t.to_string()))
            .collect(),
    }
}

#[test]
fn paired_bars_chart_golden_markup() {
    let got = draw(|p| pairs(p, "By repository", Some(&pairs_view()), true, 1));
    assert_eq!(
        got,
        r##"<figure class="chart pairs-fig" data-sortable="" data-top="1"><div class="fig-head"><figcaption>By repository</figcaption><div class="sort" role="group" hidden="" aria-label="Sort rows"><span class="sort-label">Sort by</span><button type="button" data-key="by-spend" aria-pressed="true">% spend</button><button type="button" data-key="by-sessions" aria-pressed="false">% sessions</button><button type="button" data-key="by-per-session" aria-pressed="false">Spend per session</button></div></div>
<ol class="pairs">
<li class="pair" data-by-spend="0" data-by-sessions="0" data-by-per-session="1" data-tip="acme/api: tip"><div class="pair-head"><code class="pair-label">acme/api</code><span class="pair-per">$2.50 per session</span></div><div class="pair-bars"><div class="track"><span class="bar sessions" style="--w:80"></span><span class="val"><b>60%</b> 3 sessions</span></div><div class="track"><span class="bar spend" style="--w:100"></span><span class="val"><b>75%</b> $7.50</span></div></div></li>
<li class="pair extra" data-by-spend="1" data-by-sessions="1" data-by-per-session="0" data-tip="acme/web: tip"><div class="pair-head"><code class="pair-label">acme/web</code><span class="pair-per">$2.50 per session</span></div><div class="pair-bars"><div class="track"><span class="bar sessions" style="--w:53.3"></span><span class="val"><b>40%</b> 2 sessions</span></div><div class="track"><span class="bar spend" style="--w:33.3"></span><span class="val"><b>25%</b> $2.50</span></div></div></li>
</ol>
<details class="more pairs-more"><summary><span class="when-closed">Show all 2 rows</span><span class="when-open">Show fewer</span></summary></details>
<div class="legend"><span class="key"><i class="swatch sessions"></i>% of sessions</span><span class="key"><i class="swatch spend"></i>% of spend</span></div></figure>
"##
    );
}

#[test]
fn treemap_chart_golden_svg() {
    let got = draw(|p| treemap(p, "Spend", Some(&treemap_view())));
    assert_eq!(
        got,
        r##"<figure class="chart treemap-fig"><figcaption>Spend</figcaption><div class="treemap"><svg viewBox="0 0 1000 480" preserveAspectRatio="none" role="img" aria-label="Spend"><g class="tm tone-1"><rect class="shade-1" x="0" y="0" width="500" height="480" data-tip="acme/api: $2.00 (50%)"><title>acme/api: $2.00 (50%)</title></rect><rect class="shade-2" x="500" y="0" width="250" height="480" data-tip="acme/web: $1.00 (25%)"><title>acme/web: $1.00 (25%)</title></rect><rect class="tm-org" x="0" y="0" width="750" height="480"/></g><g class="tm tone-6"><rect class="shade-1" x="750" y="0" width="250" height="480" data-tip="(unattributed): $1.00 (25%)"><title>(unattributed): $1.00 (25%)</title></rect><rect class="tm-org" x="750" y="0" width="250" height="480"/></g></svg><div class="tm-labels" aria-hidden="true"><span class="tm-label" style="--x:0;--y:0;--w:500;--h:480"><b>api</b><span>$2.00</span></span><span class="tm-label" style="--x:500;--y:0;--w:250;--h:480"><b>web</b><span>$1.00</span></span><span class="tm-label" style="--x:750;--y:0;--w:250;--h:480"><b>(unattributed)</b><span>$1.00</span></span></div></div>
<ul class="legend tm-legend"><li><i class="swatch tone-1"></i><code>acme</code> <span class="figures">$3.00 · 75%</span></li><li><i class="swatch tone-6"></i><code>(unattributed)</code> <span class="figures">$1.00 · 25%</span></li></ul></figure>
"##
    );
}

#[test]
fn stacked_bar_chart_golden_markup() {
    let got = draw(|p| stack(p, "Spend by model", Some(&stack_view())));
    assert_eq!(
        got,
        r##"<figure class="chart stack-fig"><figcaption>Spend by model</figcaption><div class="stackbar"><span class="seg tone-1" style="--x:0;--w:75" data-tip="opus: $3.00 (75%)"><span>75%</span></span><span class="seg tone-2" style="--x:75;--w:25" data-tip="haiku: $1.00 (25%)"><span>25%</span></span></div><ul class="legend stack-legend"><li><i class="swatch tone-1"></i><code>opus</code> <span class="figures">$3.00 · 75%</span></li><li><i class="swatch tone-2"></i><code>haiku</code> <span class="figures">$1.00 · 25%</span></li></ul></figure>
"##
    );
}

fn cascade(list: i64, savings: i64, spend: i64, texts: [&str; 3]) -> Cascade {
    Cascade::Drawn {
        list: texts[0].to_string(),
        savings: texts[1].to_string(),
        spend: texts[2].to_string(),
        steps: wf::waterfall(list, savings, spend).unwrap(),
    }
}

#[test]
fn waterfall_chart_golden_markup() {
    let got = draw(|p| waterfall(p, "List", &cascade(400, 300, 100, ["$4.00", "$3.00", "$1.00"])));
    assert_eq!(
        got,
        r##"<figure class="chart waterfall-fig"><figcaption>List</figcaption><ol class="steps"><li class="step list"><span class="step-label">List price</span><div class="track"><span class="bar" style="--x:0;--w:100"></span><span class="val"><b>$4.00</b></span></div></li><li class="step savings"><span class="step-label">Cache savings</span><div class="track"><span class="bar" style="--x:25;--w:75"></span><span class="val"><b>$3.00</b></span></div></li><li class="step spend"><span class="step-label">Spend</span><div class="track"><span class="bar" style="--x:0;--w:25"></span><span class="val"><b>$1.00</b></span></div></li></ol></figure>
"##
    );
}

/// Negative savings: the middle step is the premium, drawn rising from the list price to spend.
#[test]
fn negative_savings_waterfall_chart_golden_markup() {
    let got = draw(|p| waterfall(p, "List", &cascade(80, -20, 100, ["$0.80", "-$0.20", "$1.00"])));
    assert_eq!(
        got,
        r##"<figure class="chart waterfall-fig"><figcaption>List</figcaption><ol class="steps"><li class="step list"><span class="step-label">List price</span><div class="track"><span class="bar" style="--x:0;--w:80"></span><span class="val"><b>$0.80</b></span></div></li><li class="step premium"><span class="step-label">Cache premium</span><div class="track"><span class="bar" style="--x:80;--w:20"></span><span class="val"><b>-$0.20</b></span></div></li><li class="step spend"><span class="step-label">Spend</span><div class="track"><span class="bar" style="--x:0;--w:100"></span><span class="val"><b>$1.00</b></span></div></li></ol></figure>
"##
    );
}

#[test]
fn histogram_chart_golden_markup() {
    let got = draw(|p| histogram(p, "Sessions by cost", Some(&histogram_view())));
    assert_eq!(
        got,
        r##"<figure class="chart histo-fig"><figcaption>Sessions by cost</figcaption><div class="histo" style="--n:3"><div class="cols"><div class="col" data-tip="bucket 0"><span class="count">2</span><span class="colbar" style="--h:100"></span></div><div class="col" data-tip="bucket 1"><span class="count">0</span><span class="colbar" style="--h:0"></span></div><div class="col" data-tip="bucket 2"><span class="count">1</span><span class="colbar" style="--h:50"></span></div></div><div class="edges" aria-hidden="true"><span style="--i:0">$0.50</span><span style="--i:1">$1</span><span style="--i:2">$2.50</span><span style="--i:3">$5</span></div><div class="marks"><span>median</span><span></span><span>90th pct</span></div></div><p class="caption">1 session with no price is not shown.</p></figure>
"##
    );
}

#[test]
fn calendar_chart_golden_svg() {
    let got = draw(|p| calendar(p, "Spend by day", Some(&calendar_view())));
    assert_eq!(
        got,
        r##"<figure class="chart cal-fig"><figcaption>Spend by day</figcaption><div class="cal"><div class="cal-days" aria-hidden="true"><span>Mon</span><span>Tue</span><span>Wed</span><span>Thu</span><span>Fri</span><span>Sat</span><span>Sun</span></div><svg viewBox="0 0 82 22" role="img" aria-label="Spend by day"><rect class="l0" x="60" y="0" width="10" height="10" rx="2" data-tip="2026-04-04: $0.00"><title>2026-04-04: $0.00</title></rect><rect class="l4" x="72" y="0" width="10" height="10" rx="2" data-tip="2026-04-05: $4.00"><title>2026-04-05: $4.00</title></rect><rect class="l1" x="0" y="12" width="10" height="10" rx="2" data-tip="2026-04-06: $1.00"><title>2026-04-06: $1.00</title></rect></svg><div class="cal-legend" aria-hidden="true"><span>Less</span><i class="l0"></i><i class="l1"></i><i class="l2"></i><i class="l3"></i><i class="l4"></i><span>More</span></div></div></figure>
"##
    );
}

// ---- zero data: every chart draws its empty state ----------------------------------------------

#[test]
fn every_chart_has_a_zero_data_empty_state() {
    let marker = format!("<p class=\"chart-empty\">{EMPTY}</p>");
    let cases = [
        draw(|p| pairs(p, "By repository", None, true, 10)),
        draw(|p| treemap(p, "Spend", None)),
        draw(|p| stack(p, "Spend by model", None)),
        draw(|p| waterfall(p, "List", &Cascade::Empty)),
        draw(|p| histogram(p, "Sessions by cost", None)),
        draw(|p| calendar(p, "Spend by day", None)),
    ];
    for got in cases {
        assert!(got.contains(&marker), "{got}");
        assert!(!got.contains("<svg") && !got.contains("style="), "{got}");
        assert!(got.ends_with("</figure>\n"), "{got}");
    }
    let unpriced = draw(|p| waterfall(p, "List", &Cascade::Unpriced));
    assert!(unpriced.contains(WATERFALL_UNPRICED));
}

/// A tile or segment below the minimum size carries no inline label; its tooltip still does.
#[test]
fn labels_are_omitted_below_the_minimum_size_and_the_tooltip_remains() {
    let mut map = treemap_view();
    map.groups[0].tiles[1].rect = tm::Rect {
        x: 5000,
        y: 0,
        w: 1000,
        h: 4800,
    };
    map.groups[1].tiles[0].rect = tm::Rect {
        x: 7500,
        y: 0,
        w: 2000,
        h: 4800,
    };
    let got = draw(|p| treemap(p, "Spend", Some(&map)));
    assert!(
        !got.contains("<b>web</b>"),
        "a 100-unit-wide tile is too narrow for a label"
    );
    assert!(got.contains("<title>acme/web: $1.00 (25%)</title>"));
    assert!(
        got.contains("<span class=\"tm-label wide-only\""),
        "a 200-wide tile keeps its label on desktop only"
    );

    let mut s = stack_view();
    s.segments[1].segment = stk::Segment { x: 950, w: 50 };
    let got = draw(|p| stack(p, "Spend by model", Some(&s)));
    assert!(!got.contains("<span>25%</span>"));
    assert!(got.contains("data-tip=\"haiku: $1.00 (25%)\""));
    assert!(got.contains("<code>haiku</code>"), "the legend still names it");
}

/// Rows past `top_n` are marked extra, and no expander is drawn when every row fits.
#[test]
fn paired_rows_past_top_n_fold_and_a_short_list_has_no_expander() {
    let got = draw(|p| pairs(p, "By repository", Some(&pairs_view()), false, 10));
    assert!(!got.contains("pair extra"));
    assert!(!got.contains("<details"));
    assert!(got.contains("<span class=\"pair-label\">acme/api</span>"));
}

/// One row has nothing to sort.
#[test]
fn a_single_row_has_no_sort_control() {
    let mut view = pairs_view();
    view.rows.truncate(1);
    let got = draw(|p| pairs(p, "By repository", Some(&view), true, 10));
    assert!(!got.contains("class=\"sort\""));
    assert!(got.contains("<li class=\"pair\""));
}

#[test]
fn tone_classes_cover_the_palette_and_fall_back_to_muted() {
    let tones: Vec<&str> = (0..7).map(tone_class).collect();
    assert_eq!(
        tones,
        vec![
            " tone-1", " tone-2", " tone-3", " tone-4", " tone-5", " tone-6", " tone-6"
        ]
    );
}

/// Every calendar shade level and every treemap shade draws its own class.
#[test]
fn calendar_levels_and_treemap_shades_map_to_classes() {
    let mut c = calendar_view();
    c.cells[1].0.level = 2;
    c.cells[2].0.level = 3;
    let got = draw(|p| calendar(p, "Spend by day", Some(&c)));
    assert!(got.contains("<rect class=\"l2\"") && got.contains("<rect class=\"l3\""));

    let mut map = treemap_view();
    let base = map.groups[0].tiles[0].clone();
    map.groups[0].tiles = vec![base.clone(), base.clone(), base.clone(), base.clone(), base];
    let got = draw(|p| treemap(p, "Spend", Some(&map)));
    for shade in ["shade-1", "shade-2", "shade-3", "shade-4"] {
        assert!(got.contains(shade), "{shade}");
    }
    assert_eq!(
        got.matches("shade-4").count(),
        2,
        "the fifth tile reuses the lightest shade"
    );
}

#[test]
fn histogram_marks_cover_both_and_neither() {
    let mut h = histogram_view();
    h.columns[0].p90 = true;
    h.unpriced = None;
    let got = draw(|p| histogram(p, "Sessions by cost", Some(&h)));
    assert!(got.contains("<span>median, 90th pct</span>"));
    assert!(!got.contains("<p class=\"caption\">"));
}
