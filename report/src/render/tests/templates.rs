#![allow(clippy::unwrap_used)]

//! The prompt-edit ledger, as tests.
//!
//! The design doc requires that `report.pmt` and `report-html.pmt` change TOGETHER: seven phases
//! edit both, and the two formats drift the moment one is edited alone. A ledger enforced only by
//! commit review is enforced until someone is in a hurry, so each rule both templates must carry
//! gets an assertion here, over both files, naming the drift it was written for.

use crate::render::{DEFAULT_HTML_PROMPT, DEFAULT_PROMPT};

/// Phase 5's prompt-edit ledger: BOTH templates flip the agent-type framing from "attribution you
/// must never reconcile" to "a partition of the total", and keep the non-reconcilable framing for
/// the by-skill / by-mcp TAG sets only.
#[test]
fn both_templates_declare_agent_type_costs_a_partition() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            !tpl.contains("never reconcile"),
            "{name} must no longer forbid reconciling the agent-type rows"
        );
        assert!(
            tpl.contains("TRUE PARTITION of `totals.spend`"),
            "{name} must state that agent-type-costs partitions the total"
        );
        assert!(
            tpl.contains("(main-session)"),
            "{name} must name the residual row so the model can explain it"
        );
        // The tag sets keep the caveat, and gain the coverage strings that replace reconciliation.
        assert!(
            tpl.contains("cannot be reconciled against it"),
            "{name} must keep the non-reconcilable framing for by-skill / by-mcp"
        );
        assert!(
            tpl.contains("efficiency.by-skill-coverage") && tpl.contains("efficiency.by-mcp-coverage"),
            "{name} must license both coverage strings"
        );
    }
}
/// The canonical section vocabulary, spelled identically in both templates. A fixture's
/// `require-sections` / `forbid-sections` is one list checked against BOTH artifacts, so the moment
/// the templates name a section differently that spec means two different things -- which is how
/// `Forward-Looking` (markdown) and `Forward-Looking Note` (HTML) drifted apart unnoticed.
#[test]
fn both_templates_use_the_same_section_titles() {
    const SECTIONS: &[&str] = &[
        "Executive Summary",
        "Quantified Output",
        "Cost Summary",
        "Reconciliation",
        "Agent-Type Cost Attribution",
        "The Efficiency Story",
        "What This Funded",
        "Usage Profile",
        "Month over Month",
        "Tradeoffs",
        "Forward-Looking",
        "Conclusion",
    ];
    for section in SECTIONS {
        assert!(
            DEFAULT_PROMPT.contains(&format!("## {section}")),
            "report.pmt must define the `## {section}` section"
        );
        assert!(
            DEFAULT_HTML_PROMPT.contains(section),
            "report-html.pmt must name the `{section}` section in its canonical heading list"
        );
    }
    // The specific drift this test was written for.
    assert!(
        !DEFAULT_HTML_PROMPT.contains("Forward-Looking Note"),
        "report-html.pmt must use `Forward-Looking`, the name report.pmt uses"
    );
}
/// Both templates must forbid asserting a reporting CADENCE. The window is whatever `--since` /
/// `--until` made it, and the context carries no recurrence, so a render that says "this recurring
/// monthly report" states a fact nobody supplied. The prompts USED to assert it outright ("This is a
/// recurring monthly report. The reader will see a similar report every month"), which is how a
/// 7-day fixture's golden came to promise a monthly cadence and describe "the month's work".
#[test]
fn neither_template_asserts_a_reporting_cadence() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            !tpl.contains("recurring monthly report"),
            "{name} must not declare the report monthly"
        );
        assert!(
            !tpl.contains("similar report every month"),
            "{name} must not promise a monthly successor"
        );
        assert!(
            tpl.contains("imply a cadence"),
            "{name} must carry the explicit no-cadence rule"
        );
        assert!(
            tpl.contains("`period.since`") && tpl.contains("`period.until`"),
            "{name} must name the window by its own fields instead"
        );
    }
}
/// Both templates must treat a PRESENT-but-EMPTY `outcomes.totals` the same as an absent one. Fields
/// are present-if-nonzero, so an empty block means nothing was observed; the HTML render read the
/// old wording ("no `outcomes` block") literally, kept the block, and emitted a Quantified Output
/// section its fixture forbids while the markdown sibling omitted it.
#[test]
fn both_templates_omit_quantified_output_for_an_empty_outcomes_block() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            tpl.contains("present-if-nonzero"),
            "{name} must explain why an empty outcomes block means nothing was observed"
        );
        assert!(
            tpl.contains("carries no fields") || tpl.contains("carry no fields"),
            "{name} must name the present-but-empty case explicitly"
        );
    }
}
