#![allow(clippy::unwrap_used)]

use super::*;
use crate::aggregate;
use crate::persona::PersonaBlock;
use crate::render::tests::{pricing, report_with_efficiency, report_with_outcomes, sample_report};
use crate::render::{ViewOpts, document};
use crate::report::Report;

/// Build the registry the way a render does: real views over a real report.
fn registry_for(report: &Report) -> FactRegistry {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = document::build_views(report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    build(&block)
}

#[test]
fn registers_the_headline_figures_as_display_strings() {
    let reg = registry_for(&sample_report());

    assert_eq!(reg.get("totals.spend"), Some("$0.60"));
    assert_eq!(reg.get("totals.sessions"), Some("2"));
    assert_eq!(reg.get("period.since"), Some("2026-04-01"));
    assert_eq!(reg.get("period.until"), Some("2026-04-30"));
    // Inclusive calendar span, the same count `by-day` zero-fills.
    assert_eq!(reg.get("period.days"), Some("30"));
}

#[test]
fn array_facts_key_by_natural_id_with_slashes_folded() {
    let reg = registry_for(&sample_report());

    // `tatari-tv/claude-report` -> `tatari-tv-claude-report`, per the design's key grammar.
    assert!(
        reg.get("by-repo.tatari-tv-claude-report.spend").is_some(),
        "keys: {:?}",
        reg.facts.keys().collect::<Vec<_>>()
    );
    assert_eq!(reg.get("by-model.claude-opus-4-7.sessions-using"), Some("1"));
}

#[test]
fn every_key_matches_the_placeholder_grammar() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let reg = registry_for(&report);
        for key in reg.facts.keys() {
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-'),
                "key {key:?} is not matchable by [a-z0-9.-]+"
            );
        }
    }
}

/// The registry admits DISPLAY STRINGS only. A value carrying markdown structure or a placeholder
/// brace could inject either into an artifact via interpolation, so no registered value may contain
/// one. This is the "non-display leaves are a test failure" half of the design's invariant.
#[test]
fn no_registered_value_carries_structure() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let reg = registry_for(&report);
        for (key, value) in &reg.facts {
            for bad in ['|', '\n', '\r', '#', '<', '>'] {
                assert!(
                    !value.contains(bad),
                    "fact {key} value {value:?} carries the structural character {bad:?}"
                );
            }
            assert!(
                !value.contains("{{"),
                "fact {key} value {value:?} carries a placeholder open"
            );
            assert!(
                !value.contains("}}"),
                "fact {key} value {value:?} carries a placeholder close"
            );
            assert!(!value.is_empty(), "fact {key} registered an empty display string");
            assert!(
                value.len() <= MAX_FACT_BYTES,
                "fact {key} value {value:?} is over the size cap"
            );
        }
    }
}

/// A ratio the window never measured must be an ABSENT key, not the literal `n/a` -- otherwise a
/// slot could cite it and print "cache reuse stayed high at n/a".
#[test]
fn the_not_measured_sentinel_is_never_registered() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let reg = registry_for(&report);
        for (key, value) in &reg.facts {
            assert_ne!(value, NOT_MEASURED, "fact {key} registered the not-measured sentinel");
        }
    }
}

#[test]
fn outcome_counts_register_only_when_observed() {
    let bare = registry_for(&sample_report());
    assert_eq!(bare.get("outcomes.commits"), None);

    let with = registry_for(&report_with_outcomes());
    assert!(
        with.get("outcomes.commits").is_some(),
        "a report carrying an outcome rollup registers its counts"
    );
}

#[test]
fn curated_derived_keys_name_the_first_presorted_row() {
    let report = sample_report();
    let reg = registry_for(&report);
    let pricing = pricing();
    let aggregates = aggregate::compute(&report, aggregate::DEFAULT_OUTLIERS, &pricing);

    assert_eq!(reg.get("repos.top"), Some(aggregates.by_repo[0].repo.as_str()));
    assert_eq!(reg.get("repos.top-spend"), Some(aggregates.by_repo[0].spend.as_str()));
}

#[test]
fn unknown_keys_do_not_resolve() {
    let reg = registry_for(&sample_report());
    assert_eq!(reg.get("totals.spend-in-euros"), None);
    assert_eq!(reg.get(""), None);
}

#[test]
fn slug_folds_every_non_alphanumeric_run_to_one_hyphen() {
    assert_eq!(slug("tatari-tv/philo"), "tatari-tv-philo");
    assert_eq!(slug("(main-session)"), "main-session");
    assert_eq!(slug("claude-opus-4-8"), "claude-opus-4-8");
    assert_eq!(slug("Some Agent Type"), "some-agent-type");
    assert_eq!(slug("--leading-and-trailing--"), "leading-and-trailing");
}

/// Break-it proof for the duplicate-key invariant: registering a key twice is a `debug_assert`
/// failure, so a future call site that reuses a key cannot land quietly.
#[test]
#[should_panic(expected = "duplicate fact key")]
fn duplicate_keys_panic_in_debug() {
    let mut reg = FactRegistry::default();
    reg.insert("totals.spend", "$1.00");
    reg.insert("totals.spend", "$2.00");
}

/// The other half of the same guard: an over-long value is a display-string violation.
#[test]
#[should_panic(expected = "over the")]
fn oversized_values_panic_in_debug() {
    let mut reg = FactRegistry::default();
    reg.insert("basis.note", "x".repeat(MAX_FACT_BYTES + 1));
}
