#![allow(clippy::unwrap_used)]

use super::*;
use crate::aggregate;
use crate::eval::synth::{Kind, build};
use crate::persona::PersonaBlock;
use crate::render::ViewOpts;
use crate::render::document::build_views;
use crate::render::tests::{pricing, report_with_outcomes, sample_report};
use crate::report::Report;

fn with_block<R>(report: &Report, f: impl FnOnce(&ContextBlock<'_>) -> R) -> R {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = build_views(report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    f(&block)
}

fn medium() -> Report {
    build(Kind::Medium, &pricing()).unwrap()
}

#[test]
fn plural_and_quantity_agree_with_their_count() {
    assert_eq!(plural(1, "session"), "1 session");
    assert_eq!(plural(1234, "session"), "1,234 sessions");
    assert_eq!(quantity(1, "repository", "repositories"), "1 repository");
    assert_eq!(quantity(0, "repository", "repositories"), "0 repositories");
}

#[test]
fn header_figures_read_the_block() {
    with_block(&medium(), |b| {
        assert!(sessions_across(b).contains(" across "));
        assert_eq!(active_days(b), format!("{} of {}", b.period.active_days, b.period.days));
    });
}

#[test]
fn outcome_rows_are_empty_without_a_rollup_and_filled_with_one() {
    with_block(&sample_report(), |b| assert!(outcome_metric_rows(b).is_empty()));
    with_block(&report_with_outcomes(), |b| {
        let rows = outcome_metric_rows(b);
        assert!(rows.iter().any(|(l, _)| *l == "Commits"), "got {rows:?}");
    });
}

#[test]
fn repo_outcome_rows_only_cover_repos_with_outcomes() {
    with_block(&report_with_outcomes(), |b| {
        let rows = repo_outcome_rows(b);
        let with = b.aggregates.by_repo.iter().filter(|r| r.outcomes.is_some()).count();
        assert_eq!(rows.len(), with);
    });
    with_block(&sample_report(), |b| assert!(repo_outcome_rows(b).is_empty()));
}

#[test]
fn unit_cost_lines_are_ratios_never_prices() {
    with_block(&medium(), |b| {
        let lines = unit_cost_lines(b);
        assert!(!lines.is_empty());
        assert!(
            lines
                .iter()
                .all(|l| l.starts_with("A ratio of ") || l.contains("session spend"))
        );
    });
}

#[test]
fn untracked_note_names_the_models_and_the_understatement() {
    let note = untracked_models_note("`m1`");
    assert!(note.starts_with("Note: spend for the following models"));
    assert!(note.contains("`m1`") && note.contains("understates actual spend"));
}

#[test]
fn unseen_spend_sentence_quotes_every_field() {
    let r = Reconciliation {
        billed: "$10".into(),
        modeled: "$8".into(),
        delta: "$2".into(),
        source: "export.csv".into(),
        operator: "a@b.c".into(),
        window: "2026-04-01 to 2026-04-30".into(),
        scope_note: "scope".into(),
        by_model: Vec::new(),
    };
    assert_eq!(
        unseen_spend_sentence(&r),
        "Unseen account spend is $2, from export.csv for a@b.c over 2026-04-01 to 2026-04-30."
    );
}

#[test]
fn agent_type_lead_copies_total_spend_and_the_styled_top_name() {
    with_block(&medium(), |b| {
        let lead = agent_type_lead(b, "`x`");
        assert!(lead.contains(&b.totals.spend) && lead.contains("`x` carried the most"));
    });
}

#[test]
fn cache_sentences_state_the_computed_figures() {
    with_block(&medium(), |b| {
        assert!(
            cache_read_sentence(b)
                .starts_with(&format!("Cache read share was {}", b.aggregates.cache.cache_read_share))
        );
        if let Some(s) = cache_savings_sentence(b) {
            assert!(s.contains("(computed from published per-token rates)"));
        }
    });
}

#[test]
fn efficiency_signals_drop_not_measured_rates_and_zero_counts() {
    with_block(&sample_report(), |b| {
        let signals = efficiency_signals(b);
        assert!(signals.iter().all(|(_, v)| v != "n/a"));
        assert!(signals.iter().all(|(l, _)| *l != "Interrupts observed") || b.efficiency.interrupts > 0);
    });
}

#[test]
fn coverage_line_prefixes_the_statement() {
    assert_eq!(coverage_line("12 of 30"), "Coverage: 12 of 30");
}

#[test]
fn org_tiers_keep_by_org_order_and_skip_orgs_without_repos() {
    with_block(&medium(), |b| {
        let tiers = org_tiers(b);
        assert!(!tiers.is_empty());
        for t in &tiers {
            assert!(!t.repos.is_empty());
            assert!(t.repos.iter().all(|r| r.org == t.org.org));
        }
    });
}

#[test]
fn repo_lines_carry_figures_and_an_outcome_tail_only_with_outcomes() {
    with_block(&medium(), |b| {
        let row = &b.aggregates.by_repo[0];
        assert!(repo_figures(row).ends_with(&format!("{} spend)", row.spend)));
        let tail = repo_outcome_tail(row);
        assert!(tail.is_empty() || tail.starts_with(": "));
    });
    with_block(&sample_report(), |b| {
        assert_eq!(repo_outcome_tail(&b.aggregates.by_repo[0]), "");
    });
}

#[test]
fn org_summary_reads_the_row() {
    with_block(&medium(), |b| {
        let org = &b.aggregates.by_org[0];
        assert!(org_summary(org).ends_with(&format!("{}.", org.spend)));
    });
}

/// Inverts `carried_in_sentence_is_absent_without_carried_in_sessions`: the coverage line is always
/// present and reconciles the daily series with the headline; the unbucketed footnote appears only
/// when a session has no per-day buckets.
#[test]
fn day_coverage_sentence_reconciles_and_the_unbucketed_footnote_is_conditional() {
    with_block(&medium(), |b| {
        let line = day_coverage_sentence(b);
        assert!(line.contains(&b.totals.spend), "{line}");
        assert_eq!(unbucketed_sentence(b), None, "every medium session is bucketed");
    });
    let patho = build(Kind::Pathological, &pricing()).unwrap();
    with_block(&patho, |b| {
        let c = &b.aggregates.day_coverage;
        let line = day_coverage_sentence(b);
        assert!(line.contains(&c.in_window) && line.contains(&c.out_of_window), "{line}");
        assert!(line.contains("3 sessions"), "{line}");
        let foot = unbucketed_sentence(b).expect("pathological has an unbucketed session");
        assert!(
            foot.starts_with("1 session has no per-day breakdown") && foot.contains(&c.unbucketed_spend),
            "{foot}"
        );
    });
}

#[test]
fn day_rows_follow_the_series_selected() {
    with_block(&medium(), |b| {
        let spend = day_rows(&b.aggregates.by_day, true);
        let sessions = day_rows(&b.aggregates.by_day, false);
        assert_eq!(spend.len(), b.aggregates.by_day.len());
        assert_eq!(spend[0].1, b.aggregates.by_day[0].spend);
        assert_eq!(sessions[0].1, format_int(b.aggregates.by_day[0].sessions as u64));
    });
    assert!(day_rows(&[], true).is_empty());
}

#[test]
fn outlier_cells_fall_back_when_fields_are_missing() {
    with_block(&medium(), |b| {
        for row in &b.aggregates.outliers {
            assert!(!outlier_label(row).is_empty());
            let repo = outlier_repo(row);
            assert!(!repo.is_empty());
            let p = produced(row);
            assert!(!p.is_empty());
            if row.outcomes.is_none() {
                assert_eq!(p, "(no recorded output)");
            }
        }
    });
}

#[test]
fn length_mismatch_sentence_names_both_lengths() {
    let s = period_length_mismatch_sentence(14, 30);
    assert!(s.contains("covers 14 days against this period's 30"));
}

#[test]
fn movement_is_a_set_difference_in_both_directions() {
    with_block(&medium(), |b| {
        let rows = &b.aggregates.by_repo;
        let same = repo_movement(rows, rows);
        assert!(same.appeared.is_empty() && same.wound_down.is_empty());
        let fewer = &rows[1..];
        let moved = repo_movement(fewer, rows);
        assert_eq!(moved.appeared, vec![rows[0].repo.as_str()]);
        let moved = repo_movement(rows, fewer);
        assert_eq!(moved.wound_down, vec![rows[0].repo.as_str()]);
        let orgs = &b.aggregates.by_org;
        assert!(orgs_appeared(orgs, orgs).is_empty());
        assert_eq!(orgs_appeared(&orgs[1..], orgs), vec![orgs[0].org.as_str()]);
    });
}

#[test]
fn section_order_drops_tradeoffs_unless_included() {
    let report = medium();
    let pricing = pricing();
    let aggregates = aggregate::compute(&report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    for include in [false, true] {
        let opts = ViewOpts {
            include_tradeoffs: include,
            ..ViewOpts::default()
        };
        let block = build_views(&report, &aggregates, &persona, &pricing, opts).unwrap();
        let order = section_order(&block);
        assert_eq!(order.first(), Some(&Section::ExecutiveSummary));
        assert_eq!(order.last(), Some(&Section::Conclusion));
        assert_eq!(order.contains(&Section::Tradeoffs), include);
    }
}

#[test]
fn every_slot_section_names_its_slot() {
    assert_eq!(Section::ExecutiveSummary.slot(), Some("executive-summary"));
    assert_eq!(Section::Conclusion.slot(), Some("closing"));
    assert_eq!(Section::CostSummary.slot(), None);
    assert_eq!(Section::Efficiency.title(), "The Efficiency Story");
}
