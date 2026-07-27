#![allow(clippy::unwrap_used)]

//! The claim-shaped guard's own edges. The corpus-level proof that it does not fire on a correct
//! render (all six committed goldens) lives in `eval/tests.rs`, next to the other whole-artifact
//! checks.

use super::*;

fn rejected(prose: &str) -> Vec<String> {
    fabricated_claims(prose).into_iter().map(|v| v.text).collect()
}

/// The exact sentence Phase 10 planted and could not catch. `14` is a genuinely licensed count on a
/// real window (a day with 14 sessions, a repo with 14 sessions, a PR numbered 14), so the VALUE
/// guard passes it; the fabrication is the unit.
#[test]
fn the_planted_engineering_hours_claim_is_rejected() {
    let err = reject_fabricated_claims("markdown", "This funded 14 hours of engineering time.").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("14 hours"), "{msg}");
    assert!(msg.contains("no field in the context block is denominated"), "{msg}");
    assert!(msg.contains("This funded 14 hours of engineering time."), "{msg}");
}

/// The other planted claim: a multiplier is arithmetic over two figures, and the binary emits none.
#[test]
fn the_planted_multiplier_claim_is_rejected() {
    let err = reject_fabricated_claims("markdown", "Spend ran 3x the prior period.").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("3x"), "{msg}");
    assert!(msg.contains("a multiplier is arithmetic"), "{msg}");
}

/// Every unit the context block has no field for, singular and plural, abbreviated and spelled out.
#[test]
fn a_figure_in_any_unsourced_duration_unit_is_rejected() {
    for prose in [
        "roughly 40 hours of work",
        "1 hour of review",
        "12 hrs against the backlog",
        "90 minutes per session",
        "45 mins of triage",
        "30 seconds per turn",
        "6 weeks of runway",
        "2 months of effort",
        "1.5 years of maintenance",
    ] {
        assert!(!rejected(prose).is_empty(), "not rejected: {prose}");
    }
}

/// Decimal and comma-grouped magnitudes are the same claim with a bigger number.
#[test]
fn a_comma_grouped_or_decimal_duration_is_rejected() {
    assert_eq!(rejected("1,200 hours of engineering"), vec!["1,200 hours"]);
    assert_eq!(rejected("2.5 hours saved"), vec!["2.5 hours"]);
}

/// The narrowing that keeps the guard off correct renders: `1-hour` and `5-minute` are hyphenated
/// ADJECTIVES naming a cache-write tier, which `aggregates.cache` licenses. The unit is only a
/// claim when it is a noun the figure counts.
#[test]
fn a_hyphenated_cache_tier_is_not_a_duration_claim() {
    for prose in [
        "the 1-hour cache-write fraction was 3.0%",
        "5-minute cache writes carried the rest",
        "1-hour and 5-minute writes are priced differently",
    ] {
        assert!(
            rejected(prose).is_empty(),
            "false positive: {prose} -> {:?}",
            rejected(prose)
        );
    }
}

/// `days` IS in the context (`period.days`, `period.active-days`, an `N-day` window), so a bare day
/// count is legal prose and the guard must leave it alone. Every one of these appears, in shape, in
/// a committed golden.
#[test]
fn a_bare_day_count_is_legitimate() {
    for prose in [
        "**Active Days:** 29 of 30",
        "the window covers 30 days",
        "an 8-day gap sits between 2026-07-08 and 2026-07-16",
        "30 days of work across 7 repositories",
        "20 days, 12 sessions, $49.48",
    ] {
        assert!(
            rejected(prose).is_empty(),
            "false positive: {prose} -> {:?}",
            rejected(prose)
        );
    }
}

/// ...and the three shapes that turn a licensed day count into a fabricated labor claim.
#[test]
fn a_day_count_framed_as_labor_is_rejected() {
    for prose in [
        "the equivalent of 14 engineer-days",
        "3 person days of capacity",
        "2 days of senior-engineer time",
        "4 days of manual effort",
        "this saved 6 days",
        "6 days saved against the plan",
    ] {
        assert!(!rejected(prose).is_empty(), "not rejected: {prose}");
    }
}

/// The multiplier pattern only fires on a figure directly carrying `x`. A model id, a dimension, and
/// a percentage are not multipliers.
#[test]
fn a_multiplier_is_only_a_figure_immediately_carrying_x() {
    for prose in [
        "claude-opus-4-8 carried $57,734.63",
        "cache reads were 92.3% of context",
        "session 3xf9a2b1 is untitled",
        "the viewBox is 0 0 1000 300",
    ] {
        assert!(
            rejected(prose).is_empty(),
            "false positive: {prose} -> {:?}",
            rejected(prose)
        );
    }
    for prose in ["3x the prior period", "1.5x more output", "a 10X increase"] {
        assert!(!rejected(prose).is_empty(), "not rejected: {prose}");
    }
}

/// The false positive the committed goldens caught on the first run, and the reason the patterns
/// carry [`OPENS_A_FIGURE`] instead of a plain `\b`: a model name ending in a digit, followed by an
/// ordinal, is correct prose in a shipped artifact. So is a date, and so is a version.
#[test]
fn a_digit_inside_an_identifier_never_opens_a_claim() {
    for prose in [
        "claude-opus-4-7 led spend, with claude-sonnet-4-6 second and claude-haiku-4-5 third",
        "the feed's data-version is 2026-07-24 months into the year",
        "v0.14.0 minutes of the release meeting",
    ] {
        assert!(
            rejected(prose).is_empty(),
            "false positive: {prose} -> {:?}",
            rejected(prose)
        );
    }
}

/// The cache counterfactual is the ONE quantification both prompts sanction, and its prose is dense
/// with figures. The guard must not touch it.
#[test]
fn the_sanctioned_cache_counterfactual_passes() {
    let prose = "Against 1.0M input tokens, 78.5M tokens were served from cache read. The \
                 list-price equivalent of that read volume is $320.69 and the modeled cache savings \
                 is $255.84, both computed from published per-token rates.";
    reject_fabricated_claims("markdown", prose).unwrap();
}

/// Clean prose produces no error at all, and the guard says so at DEBUG rather than silently.
#[test]
fn clean_prose_passes() {
    let prose = "**Total Spend:** $9,450.31 across 1,523 sessions and 47 repositories, 29 of 30 \
                 days active, with 2026-06-30 the heaviest by-day row at $459.16.";
    reject_fabricated_claims("html", prose).unwrap();
    assert!(fabricated_claims(prose).is_empty());
}

/// Several claims in one document are reported together, so one paid render surfaces all of them
/// instead of one per retry.
#[test]
fn every_violation_in_a_document_is_reported_at_once() {
    let err = reject_fabricated_claims(
        "markdown",
        "It funded 14 hours of engineering time and ran 3x the prior period over 2 months.",
    )
    .unwrap_err();
    let msg = format!("{err}");
    for expected in ["14 hours", "2 months", "3x"] {
        assert!(msg.contains(expected), "{expected} missing from {msg}");
    }
}
