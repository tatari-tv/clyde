#![allow(clippy::unwrap_used)]

//! Tests for `render::workload`: the efficiency view's three attribution maps, and the one of them
//! that is a declared PARTITION of the headline.

use std::collections::BTreeMap;

use efficiency::WorkloadCost;

use crate::render::tests::sample_report;
use crate::render::{WorkloadRow, workload::*};
use crate::report::SessionEntry;

/// One session entry carrying `spend` and agent-type buckets `(name, cost_usd)`. Tokens are a
/// constant: they ride along and never weigh the split.
fn session(spend: Option<f64>, pairs: &[(&str, f64)]) -> SessionEntry {
    let mut entry = sample_report().sessions.into_values().next().unwrap();
    entry.spend_usd = spend;
    entry.agent_type_costs = pairs
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
        .collect();
    entry
}

fn cents_of(parts: &[(String, u64, i64)]) -> Vec<(&str, i64)> {
    parts.iter().map(|(n, _, c)| (n.as_str(), *c)).collect()
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

/// THE defect, now fixed at the source. Four buckets whose independently-rounded cents sum to
/// $671.27 under a session of $671.28 (the medium fixture's old shape): the session's own cents are
/// split across its buckets by largest remainder, so the parts ARE the session's cents and the
/// table built from them cannot miss the headline.
#[test]
fn the_agent_type_partition_reconciles_to_the_displayed_headline() {
    let entry = session(
        Some(671.28),
        &[
            ("(main-session)", 479.7449),
            ("phase-implementer", 79.3312),
            ("doc-writer", 61.7751),
            ("code-reviewer", 50.4266),
        ],
    );
    let parts = session_agent_cents(&entry);
    assert_eq!(
        parts.iter().map(|p| p.2).sum::<i64>(),
        67128,
        "the session's cents, exactly"
    );
    // The two residual cents land on the largest discarded fractions (.68 and .65).
    assert_eq!(
        cents_of(&parts),
        vec![
            ("(main-session)", 47975),
            ("code-reviewer", 5043),
            ("doc-writer", 6177),
            ("phase-implementer", 7933),
        ]
    );
}

/// A shortfall is taken back, not handed out: the parts must not exceed the session either.
#[test]
fn the_partition_takes_a_cent_back_from_the_smallest_remainder() {
    let parts = session_agent_cents(&session(Some(15.0), &[("a", 10.009), ("b", 5.001)]));
    assert_eq!(cents_of(&parts), vec![("a", 1000), ("b", 500)]);
}

/// Inverted from `a_gap_larger_than_rounding_is_left_visible_not_smeared`. The rows used to be
/// reallocated against the headline at display time and declined on a gap larger than rounding,
/// leaving a table that did not sum. Now the session's cents are the authority and its buckets only
/// say how to divide them, so even buckets that disagree with the session sum to its cents.
#[test]
fn buckets_that_disagree_with_the_session_still_split_exactly_its_cents() {
    let parts = session_agent_cents(&session(Some(0.60), &[("implementer", 9.00), ("researcher", 4.50)]));
    assert_eq!(cents_of(&parts), vec![("implementer", 40), ("researcher", 20)]);
}

/// A session with spend and no buckets puts its cents on `(main-session)`; an unpriced session
/// with no buckets contributes no row.
#[test]
fn spend_with_no_bucket_lands_on_the_main_session_row() {
    assert_eq!(
        cents_of(&session_agent_cents(&session(Some(1.23), &[]))),
        vec![(crate::report::MAIN_SESSION_BUCKET, 123)]
    );
    assert!(session_agent_cents(&session(None, &[])).is_empty());
    assert_eq!(
        cents_of(&session_agent_cents(&session(None, &[("a", 2.0)]))),
        vec![("a", 0)],
        "an unpriced session keeps its bucket at zero cents"
    );
}

/// Rows are sums of per-session cents, in cents-descending order with a name tiebreak, and
/// deterministic.
#[test]
fn the_partition_keeps_display_order_and_is_deterministic() {
    let map = |pairs: &[(&str, i64)]| -> BTreeMap<String, AgentCents> {
        pairs
            .iter()
            .map(|(n, c)| {
                (
                    (*n).to_string(),
                    AgentCents {
                        tokens: 1_000,
                        cents: *c,
                    },
                )
            })
            .collect()
    };
    let input = [("zulu", 101), ("alpha", 101), ("big", 9000)];
    let first = partition_rows(map(&input));
    let second = partition_rows(map(&input));
    assert_eq!(
        first.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["big", "alpha", "zulu"],
        "spend descending, then name ascending"
    );
    assert_eq!(spends(&first), spends(&second), "same input, same cents, every run");
    assert_eq!(spends(&first), vec!["$90.00", "$1.01", "$1.01"]);
    assert_eq!(displayed_cents(&first), 9202);
}

#[test]
fn an_empty_partition_renders_no_rows() {
    assert!(partition_rows(BTreeMap::new()).is_empty());
}

/// A zero-spend window renders `0.0%`, never a `NaN` percent -- matching `compute_attribution`'s
/// precedent for the same divide-by-zero.
#[test]
fn coverage_note_on_a_zero_total_is_zero_percent_not_nan() {
    assert_eq!(coverage_note(0.0, 0.0), "$0.00 of $0.00 (0.0%), embedded-price basis");
}
