#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::*;
use crate::aggregate::{self as agg, Aggregates};
use crate::eval::fixture::{self, Fixture};
use crate::persona::PersonaBlock;
use crate::render::ViewOpts;
use crate::render::document::build_views;
use crate::render::tests::pricing;
use crate::report::Report;

fn fixtures() -> Vec<(String, Report)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    fixture::committed_dirs(&root)
        .iter()
        .map(|dir| {
            let f = Fixture::load(dir).unwrap();
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            (name, crate::render::load_report(&f.report, "fixture").unwrap())
        })
        .collect()
}

fn report(name: &str) -> Report {
    fixtures().into_iter().find(|(n, _)| n == name).unwrap().1
}

/// Run `f` over the block for `report`, its aggregates optionally adjusted first.
fn with_block<R>(report: &Report, adjust: impl FnOnce(&mut Aggregates), f: impl FnOnce(&ContextBlock<'_>) -> R) -> R {
    let pricing = pricing();
    let mut aggregates = agg::compute(report, agg::DEFAULT_OUTLIERS, &pricing);
    adjust(&mut aggregates);
    let persona = PersonaBlock::default();
    let opts = ViewOpts {
        include_tradeoffs: true,
        prior: None,
        reconcile: None,
        reconcile_user: None,
        ..ViewOpts::default()
    };
    let block = build_views(report, &aggregates, &persona, &pricing, opts).unwrap();
    f(&block)
}

// ---- the waterfall reconciles to the displayed figures -----------------------------------------

/// Design Phase 3 criterion: the waterfall's list price minus its savings IS the spend display
/// string, on every fixture, to the cent.
#[test]
fn waterfall_list_price_minus_savings_equals_the_spend_display_string() {
    let mut drawn = 0;
    for (name, report) in fixtures() {
        with_block(
            &report,
            |_| {},
            |block| {
                let cache = &block.aggregates.cache;
                let (Some(list), Some(savings)) = (cache.list_price_cents, cache.cache_savings_cents) else {
                    // The pathological fixture carries an unpriced cache-bearing model.
                    assert!(matches!(build(block).waterfall, Cascade::Unpriced), "{name}");
                    return;
                };
                drawn += 1;
                assert_eq!(
                    format_usd(crate::cents::to_dollars(list - savings)),
                    block.totals.spend,
                    "{name}"
                );
                let Cascade::Drawn {
                    list: list_text,
                    savings: savings_text,
                    spend,
                    ..
                } = build(block).waterfall
                else {
                    panic!("{name}: waterfall not drawn");
                };
                assert_eq!(&spend, &block.totals.spend, "{name}");
                assert_eq!(list_text, format_usd(crate::cents::to_dollars(list)), "{name}");
                assert_eq!(savings_text, format_usd(crate::cents::to_dollars(savings)), "{name}");
            },
        );
    }
    assert_eq!(drawn, 2, "small and medium are fully priced");
}

/// The 1h write premium outweighs the read savings: savings go negative and the step rises.
#[test]
fn negative_savings_draw_a_rising_waterfall_step() {
    with_block(
        &report("medium"),
        |a| {
            a.cache.list_price_cents = Some(a.cache.spend_cents - 2_00);
            a.cache.cache_savings_cents = Some(-2_00);
        },
        |block| {
            let Cascade::Drawn { steps, .. } = build(block).waterfall else {
                panic!("waterfall not drawn");
            };
            assert!(steps.savings_rise);
            assert_eq!(steps.spend.width, crate::chart::FULL);
        },
    );
}

#[test]
fn an_unpriced_counterfactual_is_not_drawn() {
    with_block(
        &report("medium"),
        |a| {
            a.cache.list_price_cents = None;
            a.cache.cache_savings_cents = None;
            a.cache.list_price_equivalent = None;
            a.cache.cache_savings = None;
        },
        |block| assert!(matches!(build(block).waterfall, Cascade::Unpriced)),
    );
}

#[test]
fn a_waterfall_with_nothing_to_draw_is_empty() {
    with_block(
        &report("medium"),
        |a| {
            a.cache.list_price_cents = Some(0);
            a.cache.cache_savings_cents = Some(0);
            a.cache.spend_cents = 0;
        },
        |block| assert!(matches!(build(block).waterfall, Cascade::Empty)),
    );
}

// ---- display strings ---------------------------------------------------------------------------

#[test]
fn share_is_a_whole_percent_with_a_floor_and_no_nan() {
    assert_eq!(share(39.4, 100.0), "39%");
    assert_eq!(share(39.5, 100.0), "40%");
    assert_eq!(share(0.4, 100.0), "<1%");
    assert_eq!(share(0.5, 100.0), "1%");
    assert_eq!(share(0.0, 100.0), "0%");
    assert_eq!(share(5.0, 0.0), "n/a");
    assert_eq!(share(100.0, 100.0), "100%");
}

#[test]
fn ratio_is_zero_on_a_zero_denominator() {
    assert_eq!(ratio(3.0, 4.0), 0.75);
    assert_eq!(ratio(3.0, 0.0), 0.0);
}

#[test]
fn ranks_sort_descending_and_keep_ties_in_input_order() {
    assert_eq!(ranks(&[1.0, 5.0, 3.0, 5.0]), vec![3, 0, 2, 1]);
    assert!(ranks(&[]).is_empty());
}

#[test]
fn tones_cycle_and_unattributed_is_muted() {
    assert_eq!(tone(0, "acme"), 0);
    assert_eq!(tone(TONES + 1, "acme"), 1);
    assert_eq!(tone(2, UNATTRIBUTED_ORG), MUTED_TONE);
}

// ---- each chart's view over a real fixture -----------------------------------------------------

#[test]
fn repo_pairs_carry_both_shares_and_three_sort_ranks() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let v = build(block);
            let repos = v.repos.unwrap();
            assert_eq!(repos.rows.len(), block.aggregates.by_repo.len());
            let quill = &repos.rows[0];
            assert_eq!(quill.label, "openpipe-oss/quill");
            assert_eq!(quill.count, "17 sessions");
            assert_eq!(quill.count_share, "39%");
            assert_eq!(quill.spend, "$264.78");
            assert_eq!(quill.spend_share, "39%");
            assert_eq!(
                quill.bars.second,
                crate::chart::FULL,
                "the largest share fills the track"
            );
            assert_eq!(
                quill.tip,
                "openpipe-oss/quill: 17 sessions (39%), $264.78 (39%), $15.58 per session"
            );
            assert_eq!(quill.rank.spend, 0);
            assert_eq!(quill.rank.count, 0);
            let spend_ranks: Vec<usize> = repos.rows.iter().map(|r| r.rank.spend).collect();
            assert_eq!(spend_ranks, (0..repos.rows.len()).collect::<Vec<_>>());
            let mut per: Vec<usize> = repos.rows.iter().map(|r| r.rank.per_unit).collect();
            per.sort_unstable();
            assert_eq!(per, (0..repos.rows.len()).collect::<Vec<_>>());
            assert_eq!(v.orgs.unwrap().rows.len(), block.aggregates.by_org.len());
        },
    );
}

#[test]
fn the_treemap_has_one_group_per_org_and_a_tile_per_repo() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let map = build(block).treemap.unwrap();
            assert_eq!(map.groups.len(), block.aggregates.by_org.len());
            let tiles: usize = map.groups.iter().map(|g| g.tiles.len()).sum();
            assert_eq!(tiles, block.aggregates.by_repo.len());
            let first = &map.groups[0];
            assert_eq!(first.label, "northwind-media");
            assert_eq!(first.share, "53%");
            assert_eq!(first.tone, 0);
            let beacon = first.tiles.iter().find(|t| t.label == "beacon").unwrap();
            assert_eq!(beacon.spend, "$165.84");
            assert_eq!(beacon.tip, "northwind-media/beacon: $165.84 (25%)");
        },
    );
}

/// An org with no repo rows (unattributed sessions) is one tile, labeled with the org.
#[test]
fn an_org_with_no_repo_rows_is_one_muted_tile() {
    with_block(
        &report("medium"),
        |a| {
            let first = a.by_org[0].clone();
            a.by_repo.retain(|r| r.org != first.org);
            a.by_org[0].org = UNATTRIBUTED_ORG.to_string();
        },
        |block| {
            let map = build(block).treemap.unwrap();
            let group = &map.groups[0];
            assert_eq!(group.tone, MUTED_TONE);
            assert_eq!(group.tiles.len(), 1);
            assert_eq!(group.tiles[0].label, UNATTRIBUTED_ORG);
            assert_eq!(group.tiles[0].rect, group.rect);
            assert_eq!(group.tiles[0].tip, format!("{UNATTRIBUTED_ORG}: $353.04 (53%)"));
        },
    );
}

#[test]
fn the_model_stack_has_a_segment_per_priced_model() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let stack = build(block).models.unwrap();
            let labels: Vec<&str> = stack.segments.iter().map(|s| s.label.as_str()).collect();
            assert_eq!(labels, vec!["claude-opus-4-7", "claude-sonnet-4-6", "claude-haiku-4-5"]);
            assert_eq!(stack.segments[0].share, "58%");
            assert_eq!(stack.segments[0].tip, "claude-opus-4-7: $386.51 (58%)");
            assert_eq!(stack.segments[2].tone, 2);
            let widths: i64 = stack.segments.iter().map(|s| s.segment.w).sum();
            assert_eq!(widths, crate::chart::FULL);
        },
    );
}

#[test]
fn the_histogram_counts_every_priced_session_once() {
    for (name, report) in fixtures() {
        with_block(
            &report,
            |_| {},
            |block| {
                let h = build(block).histogram.unwrap();
                let counted: usize = h
                    .columns
                    .iter()
                    .map(|c| c.count.replace(',', "").parse::<usize>().unwrap())
                    .sum();
                let priced = block.sessions.iter().filter(|s| s.spend_raw.is_some()).count();
                assert_eq!(counted, priced, "{name}");
                assert_eq!(h.columns.iter().filter(|c| c.median).count(), 1, "{name}");
                assert_eq!(h.columns.iter().filter(|c| c.p90).count(), 1, "{name}");
                assert!(
                    h.edges.len() == h.columns.len() + 1,
                    "{name}: no fixture reaches the open bucket"
                );
                let unpriced = block.sessions.len() - priced;
                assert_eq!(h.unpriced.is_some(), unpriced > 0, "{name}");
            },
        );
    }
}

#[test]
fn medium_histogram_marks_the_median_and_ninetieth_buckets() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let h = build(block).histogram.unwrap();
            assert_eq!(h.edges.first().unwrap(), "$1");
            assert_eq!(h.edges.last().unwrap(), "$100");
            let median = h.columns.iter().find(|c| c.median).unwrap();
            assert_eq!(median.tip, "$10 to $25: 18 sessions");
            let p90 = h.columns.iter().find(|c| c.p90).unwrap();
            assert_eq!(p90.tip, "$25 to $50: 8 sessions");
        },
    );
}

#[test]
fn unpriced_sessions_are_named_not_bucketed() {
    let mut report = report("small");
    let key = report.sessions.keys().next().unwrap().clone();
    report.sessions.get_mut(&key).unwrap().spend_usd = None;
    with_block(
        &report,
        |_| {},
        |block| {
            let h = build(block).histogram.unwrap();
            assert_eq!(h.unpriced.as_deref(), Some("1 session with no price is not shown."));
        },
    );
    let second = report.sessions.keys().nth(1).unwrap().clone();
    report.sessions.get_mut(&second).unwrap().spend_usd = None;
    with_block(
        &report,
        |_| {},
        |block| {
            let h = build(block).histogram.unwrap();
            assert_eq!(h.unpriced.as_deref(), Some("2 sessions with no price are not shown."));
        },
    );
}

#[test]
fn the_open_last_bucket_reads_and_up() {
    let mut report = report("small");
    let key = report.sessions.keys().next().unwrap().clone();
    report.sessions.get_mut(&key).unwrap().spend_usd = Some(1_500.0);
    with_block(
        &report,
        |_| {},
        |block| {
            let h = build(block).histogram.unwrap();
            assert_eq!(h.columns.last().unwrap().tip, "$1,000 and up: 1 session");
            assert_eq!(h.edges.len(), h.columns.len(), "an open bucket has no upper edge");
        },
    );
}

#[test]
fn the_calendar_has_a_cell_and_tip_per_day() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let cal = build(block).calendar.unwrap();
            assert_eq!(cal.cells.len(), block.aggregates.by_day.len());
            assert_eq!(cal.cells[0].1, "2026-04-01: $11.80");
            assert_eq!(cal.height, 58);
        },
    );
}

// ---- zero data ---------------------------------------------------------------------------------

#[test]
fn zero_data_draws_no_chart() {
    let mut empty = report("small");
    empty.sessions.clear();
    with_block(
        &empty,
        |a| {
            a.by_org.clear();
            a.by_repo.clear();
            a.by_day.clear();
            a.cache.list_price_cents = Some(0);
            a.cache.cache_savings_cents = Some(0);
            a.cache.spend_cents = 0;
        },
        |block| {
            let v = build(block);
            assert!(v.orgs.is_none());
            assert!(v.repos.is_none());
            assert!(v.treemap.is_none());
            assert!(v.histogram.is_none());
            assert!(v.calendar.is_none());
            assert!(matches!(v.waterfall, Cascade::Empty));
        },
    );
}

#[test]
fn unpriced_models_draw_no_model_stack() {
    let mut report = report("small");
    for m in report.totals.models.values_mut() {
        m.spend_usd = None;
    }
    with_block(&report, |_| {}, |block| assert!(build(block).models.is_none()));
}

#[test]
fn an_unparseable_first_day_draws_no_calendar() {
    with_block(
        &report("medium"),
        |a| a.by_day[0].date = "not-a-date".to_string(),
        |block| assert!(build(block).calendar.is_none()),
    );
}

#[test]
fn display_strings_cover_every_chart_label() {
    with_block(
        &report("medium"),
        |_| {},
        |block| {
            let strings = display_strings(&build(block));
            for expected in [
                "$264.78",
                "39%",
                "17 sessions",
                "$10 to $25: 18 sessions",
                "$3,201.81",
                "2026-04-01: $11.80",
            ] {
                assert!(strings.iter().any(|s| s == expected), "{expected}");
            }
        },
    );
}

// ---- calls charts keep the table's row coverage ------------------------------------------------

fn call_row(name: &str, calls: u64, spend: f64, partial: bool) -> crate::render::CallRow {
    crate::render::CallRow {
        name: name.to_string(),
        server: None,
        calls: Some(String::new()),
        sessions: Some(String::new()),
        tokens_human: String::new(),
        spend: format_usd(spend),
        per_call: Some(String::new()),
        time: None,
        avg: None,
        counts: Some(crate::render::CallCounts {
            calls,
            spend,
            counted_spend: spend,
            partial,
        }),
    }
}

/// A partial row's count label and tip carry `\u{2020}` the way its table cell does, a counted row's
/// do not, and a partial row with 0 counted calls (the table's `n/a`) is not drawn. BITES: drop the
/// flag from `CallCounts` and the chart says `3 calls` and `0 calls (0%)`.
#[test]
fn partial_call_rows_carry_the_dagger_and_na_rows_are_not_drawn() {
    let rows = [
        call_row("counted", 4, 2.0, false),
        call_row("partial", 3, 1.0, true),
        call_row("none-counted", 0, 1.0, true),
    ];
    let pairs = call_pairs(&rows).unwrap();
    let labels: Vec<&str> = pairs.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["counted", "partial"], "the n/a row has no share to draw");
    assert_eq!(pairs.rows[0].count, "4 calls");
    assert!(
        pairs.rows[0].tip.starts_with("counted: 4 calls (57%)"),
        "{}",
        pairs.rows[0].tip
    );
    assert_eq!(pairs.rows[1].count, "3\u{2020} calls");
    assert!(
        pairs.rows[1].tip.starts_with("partial: 3\u{2020} calls (43%)"),
        "{}",
        pairs.rows[1].tip
    );
}

/// Every call row partial with 0 counted calls: nothing to draw, so no figure.
#[test]
fn a_calls_chart_of_only_na_rows_is_not_drawn() {
    assert!(call_pairs(&[call_row("a", 0, 1.0, true)]).is_none());
}

/// A count-less row (the agent table's `(main-session)`) is skipped, not fatal; shares are of the
/// counted rows; a table where every row is count-less draws nothing.
#[test]
fn call_pairs_skips_countless_rows_and_none_when_all_are() {
    let mut countless = call_row("(main-session)", 0, 0.0, false);
    countless.counts = None;
    let mut countless_too = call_row("other", 0, 0.0, false);
    countless_too.counts = None;
    let mut mixed_countless = call_row("(main-session)", 0, 0.0, false);
    mixed_countless.counts = None;
    let mixed = [
        call_row("a", 3, 6.0, false),
        mixed_countless,
        call_row("b", 1, 2.0, false),
    ];
    let pairs = call_pairs(&mixed).expect("counted rows draw");
    let labels: Vec<&str> = pairs.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["a", "b"]);
    assert_eq!(pairs.rows[0].count_share, "75%");
    assert_eq!(pairs.rows[1].count_share, "25%");
    assert!(call_pairs(&[countless, countless_too]).is_none());
}
