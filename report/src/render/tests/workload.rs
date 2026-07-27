#![allow(clippy::unwrap_used)]

//! Tests for `render::workload`: the efficiency view's three attribution maps, and the one of them
//! that is a declared PARTITION of the headline.

use std::collections::BTreeMap;

use efficiency::WorkloadCost;

use crate::render::{WorkloadRow, workload::*};

/// Build an agent-type bucket map from `(name, cost_usd)` pairs. Tokens are irrelevant to the
/// residual allocation, so they are a constant.
fn buckets(pairs: &[(&str, f64)]) -> BTreeMap<String, WorkloadCost> {
    pairs
        .iter()
        .map(|(name, cost)| {
            (
                (*name).to_string(),
                WorkloadCost {
                    tokens: 1_000,
                    cost_usd: *cost,
                },
            )
        })
        .collect()
}

fn spends(rows: &[WorkloadRow]) -> Vec<&str> {
    rows.iter().map(|r| r.spend.as_str()).collect()
}

/// Sum a set of `$X.XX` display strings back into cents, the way a reader adds a table up.
fn displayed_cents(rows: &[WorkloadRow]) -> i64 {
    rows.iter()
        .map(|r| {
            let cleaned = r.spend.replace(['$', ','], "");
            (cleaned.parse::<f64>().unwrap() * 100.0).round() as i64
        })
        .sum()
}

/// THE defect. Four buckets whose independently-rounded cents sum to $671.27 under a headline of
/// $671.28, which is the medium fixture's exact shape: the artifact asserted "every dollar landing
/// in exactly one row and totaling $671.28" above rows that totaled a cent less.
#[test]
fn the_agent_type_partition_reconciles_to_the_displayed_headline() {
    let rows = partition_rows(
        buckets(&[
            ("(main-session)", 479.7449),
            ("phase-implementer", 79.3312),
            ("doc-writer", 61.7751),
            ("code-reviewer", 50.4266),
        ]),
        671.2778,
    );
    assert_eq!(
        displayed_cents(&rows),
        67128,
        "the displayed rows must add up to the displayed headline: {:?}",
        spends(&rows)
    );
    // The residual cent lands on the largest discarded fraction (.66), not on an arbitrary row.
    assert_eq!(
        spends(&rows),
        vec!["$479.74", "$79.33", "$61.78", "$50.43"],
        "largest remainder first"
    );
}

/// A shortfall is taken back, not handed out: the rows must not exceed the headline either.
#[test]
fn the_partition_takes_a_cent_back_from_the_smallest_remainder() {
    let rows = partition_rows(buckets(&[("a", 10.009), ("b", 5.001)]), 15.0);
    assert_eq!(displayed_cents(&rows), 1500, "got {:?}", spends(&rows));
    assert_eq!(
        spends(&rows),
        vec!["$10.00", "$5.00"],
        "the cent comes off `b`, whose .1 remainder is the smallest"
    );
}

/// Allocation is capped at one cent per bucket -- the most flooring can lose. A gap bigger than that
/// is a genuine disagreement between the buckets and the headline, and smearing it across the rows
/// would hide a real defect behind a table that adds up. Render the measured figures instead.
#[test]
fn a_gap_larger_than_rounding_is_left_visible_not_smeared() {
    let rows = partition_rows(buckets(&[("implementer", 9.00), ("researcher", 4.50)]), 0.60);
    assert_eq!(
        spends(&rows),
        vec!["$9.00", "$4.50"],
        "the measured figures survive untouched"
    );
    assert_ne!(
        displayed_cents(&rows),
        60,
        "and the rows deliberately do NOT sum to a headline they never supported"
    );
}

/// Display order is spend-descending with a name tiebreak, and the allocation must not disturb it.
#[test]
fn the_partition_keeps_display_order_and_is_deterministic() {
    let input = [("zulu", 1.005), ("alpha", 1.005), ("big", 90.0)];
    let first = partition_rows(buckets(&input), 92.01);
    let second = partition_rows(buckets(&input), 92.01);
    assert_eq!(
        first.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["big", "alpha", "zulu"],
        "spend descending, then name ascending"
    );
    assert_eq!(spends(&first), spends(&second), "same input, same cents, every run");
    assert_eq!(displayed_cents(&first), 9201);
}

#[test]
fn an_empty_partition_renders_no_rows() {
    assert!(partition_rows(BTreeMap::new(), 0.0).is_empty());
    assert!(
        partition_rows(BTreeMap::new(), 12.34).is_empty(),
        "a total with no buckets to carry it warns, it does not invent a row"
    );
}
/// A zero-spend window renders `0.0%`, never a `NaN` percent -- matching `compute_attribution`'s
/// precedent for the same divide-by-zero.
#[test]
fn coverage_note_on_a_zero_total_is_zero_percent_not_nan() {
    assert_eq!(coverage_note(0.0, 0.0), "$0.00 of $0.00 (0.0%), embedded-price basis");
}
