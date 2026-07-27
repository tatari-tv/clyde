#![allow(clippy::unwrap_used)]

//! The claim-shaped guard's own edges. The corpus-level proof that it does not fire on a correct
//! render (all six committed goldens) lives in `eval/tests.rs`, next to the other whole-artifact
//! checks.

use super::*;
use crate::quotable::QuotableFacts;

/// A context licensing NOTHING: every edge below is about the claim patterns themselves, so the
/// identifier exemption must not quietly cover a planted claim. The one test that DOES exercise the
/// exemption builds its own context.
fn no_facts() -> QuotableFacts {
    QuotableFacts::from_context_json("{}").unwrap()
}

fn rejected(prose: &str) -> Vec<String> {
    fabricated_claims(prose, &no_facts())
        .into_iter()
        .map(|v| v.text)
        .collect()
}

/// The exact sentence Phase 10 planted and could not catch. `14` is a genuinely licensed count on a
/// real window (a day with 14 sessions, a repo with 14 sessions, a PR numbered 14), so the VALUE
/// guard passes it; the fabrication is the unit.
#[test]
fn the_planted_engineering_hours_claim_is_rejected() {
    let err =
        reject_fabricated_claims("markdown", "This funded 14 hours of engineering time.", &no_facts()).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("14 hours"), "{msg}");
    assert!(msg.contains("no field in the context block is denominated"), "{msg}");
    assert!(msg.contains("This funded 14 hours of engineering time."), "{msg}");
}

/// The other planted claim: a multiplier is arithmetic over two figures, and the binary emits none.
#[test]
fn the_planted_multiplier_claim_is_rejected() {
    let err = reject_fabricated_claims("markdown", "Spend ran 3x the prior period.", &no_facts()).unwrap_err();
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
    reject_fabricated_claims("markdown", prose, &no_facts()).unwrap();
}

/// Clean prose produces no error at all, and the guard says so at DEBUG rather than silently.
#[test]
fn clean_prose_passes() {
    let prose = "**Total Spend:** $9,450.31 across 1,523 sessions and 47 repositories, 29 of 30 \
                 days active, with 2026-06-30 the heaviest by-day row at $459.16.";
    reject_fabricated_claims("html", prose, &no_facts()).unwrap();
    assert!(fabricated_claims(prose, &no_facts()).is_empty());
}

/// Several claims in one document are reported together, so one paid render surfaces all of them
/// instead of one per retry.
#[test]
fn every_violation_in_a_document_is_reported_at_once() {
    let err = reject_fabricated_claims(
        "markdown",
        "It funded 14 hours of engineering time and ran 3x the prior period over 2 months.",
        &no_facts(),
    )
    .unwrap_err();
    let msg = format!("{err}");
    for expected in ["14 hours", "2 months", "3x"] {
        assert!(msg.contains(expected), "{expected} missing from {msg}");
    }
}

/// A context whose enrich `summary` legitimately contains a duration. `summary` is classified
/// `Identifier` precisely so the prose may quote it, and both prompts instruct it to.
fn facts_with_a_duration_in_a_summary() -> QuotableFacts {
    QuotableFacts::from_context_json(
        r#"{"sessions": [{"short-id": "a14bc3d2",
             "summary": "Spent 3 hours chasing the flake before finding the clock skew."}]}"#,
    )
    .unwrap()
}

/// The claim guard must apply the SAME identifier exemption the value guard does. Without it, a
/// render that quoted an enrich summary verbatim -- exactly what the prompts tell it to do -- was a
/// HARD failure on a paid call, and the two guards disagreed about the same sentence.
#[test]
fn a_duration_inside_a_quoted_summary_is_a_citation_not_a_fabrication() {
    let facts = facts_with_a_duration_in_a_summary();
    let cited = "Session a14bc3d2 reported: \"Spent 3 hours chasing the flake before finding the \
                 clock skew.\" That was the window's longest single thread.";

    assert!(
        fabricated_claims(cited, &facts).is_empty(),
        "quoting a summary the context carries is a citation: {:?}",
        fabricated_claims(cited, &facts)
    );
    // And the value guard already agreed, which is the disagreement this closes.
    assert!(
        facts.foreign_figures(cited).is_empty(),
        "the two guards must reach the same verdict on the same sentence"
    );
}

/// BITES: the exemption is span-scoped, not a blanket amnesty. The same duration OUTSIDE the quoted
/// span is still a fabrication, so a model cannot launder a claim by citing an unrelated summary.
#[test]
fn the_same_duration_outside_the_citation_is_still_rejected() {
    let facts = facts_with_a_duration_in_a_summary();
    let planted = "Session a14bc3d2 reported: \"Spent 3 hours chasing the flake before finding the \
                   clock skew.\" Across the window this saved 3 hours of engineering time.";

    let claims: Vec<String> = fabricated_claims(planted, &facts).into_iter().map(|v| v.text).collect();
    assert_eq!(
        claims,
        vec!["3 hours".to_string()],
        "the unquoted claim must still be caught exactly once"
    );
}

/// A figure ending one block and a duration word opening another are NOT a claim. `\s+` crossed
/// blank lines, so a table cell reading `$24.98` and a later `## Month over Month` heading matched
/// as "24.98 months" -- refusing a correct HTML render, and a paid call with it.
#[test]
fn a_figure_and_a_unit_in_different_blocks_are_not_a_claim() {
    let across_blocks = "| northwind-media/almanac | 23.7M | $24.98 |\n\n\n## Month over Month\n\n\
                         The prior period ran 2026-03-02 to 2026-03-31.";
    assert!(
        rejected(across_blocks).is_empty(),
        "a table cell and a later heading are not a duration claim: {:?}",
        rejected(across_blocks)
    );

    // The HTML shape that actually failed: whitespace-only lines between the two.
    let html_shaped = "$24.98  \n       \n     \n   \n \n\n \n   Month over Month";
    assert!(rejected(html_shaped).is_empty(), "{:?}", rejected(html_shaped));
}

/// BITES: the narrowing must not create a false NEGATIVE. Prose legitimately breaks a line between
/// a number and its unit, and that is still one claim.
#[test]
fn a_claim_wrapped_across_a_single_line_is_still_rejected() {
    for prose in [
        "This funded 14\nhours of engineering time.",
        "This funded 14 \n  hours of engineering time.",
        "the equivalent of 3\ndays of senior-engineer time",
    ] {
        assert!(
            !rejected(prose).is_empty(),
            "a soft wrap must not launder a claim: {prose:?}"
        );
    }
}
