#![allow(clippy::unwrap_used)]

use std::cell::RefCell;

use eyre::bail;

use super::*;
use crate::aggregate;
use crate::persona::PersonaBlock;
use crate::render::tests::{pricing, report_with_efficiency, sample_report, ts};
use crate::render::{ViewOpts, document};
use crate::report::Report;

/// What one transport call was handed. A fake that RECORDS, never a mock.
#[derive(Debug, Clone)]
struct Recorded {
    kind: Kind,
    model: String,
    ceiling: u32,
    system: String,
    prompt: String,
    brief: String,
}

/// Returns a scripted reply per call, so a test can drive the retry ladder deterministically.
struct Scripted {
    replies: RefCell<Vec<String>>,
    seen: RefCell<Vec<Recorded>>,
}

impl Scripted {
    /// Every slot gets the same reply, forever.
    fn always(reply: &str) -> Self {
        Self {
            replies: RefCell::new(vec![reply.to_string()]),
            seen: RefCell::new(Vec::new()),
        }
    }

    /// Replies are consumed in order; the last one repeats once exhausted.
    fn sequence(replies: &[&str]) -> Self {
        Self {
            replies: RefCell::new(replies.iter().rev().map(|s| s.to_string()).collect()),
            seen: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Recorded> {
        self.seen.borrow().clone()
    }
}

impl Transport for Scripted {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, brief: &str) -> eyre::Result<String> {
        self.seen.borrow_mut().push(Recorded {
            kind: job.kind,
            model: job.model.to_string(),
            ceiling: job.max_output_tokens,
            system: system.to_string(),
            prompt: prompt.to_string(),
            brief: brief.to_string(),
        });
        let mut replies = self.replies.borrow_mut();
        Ok(if replies.len() > 1 {
            replies.pop().unwrap()
        } else {
            replies.last().cloned().unwrap_or_default()
        })
    }
}

struct Exploding;

impl Transport for Exploding {
    fn complete(&self, _: Job<'_>, _: &str, _: &str, _: &str) -> eyre::Result<String> {
        bail!("transport exploded")
    }
}

const MODEL: &str = "claude-opus-4-8";
const CEILING: u32 = 1_500;

fn registry_for(report: &Report) -> FactRegistry {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = document::build_views(report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    document::registry(&block)
}

/// A conforming reply for the `executive-summary` slot: digit-free, allowlisted placeholders only.
const GOOD: &str = "Across {{fact:period.days}} days this account ran {{fact:totals.sessions}} sessions for \
     {{fact:totals.spend}}, concentrated in {{fact:repos.top}}.";

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

#[test]
fn a_conforming_reply_is_accepted() {
    let allow = Slot::ExecutiveSummary.allowlist();
    assert_eq!(validate(GOOD, allow).unwrap(), GOOD);
}

#[test]
fn a_bare_digit_is_rejected() {
    let allow = Slot::ExecutiveSummary.allowlist();
    let err = validate("This period ran 1,527 sessions.", allow).unwrap_err();
    assert!(matches!(err, Violation::Numeric('1')), "got {err:?}");
}

/// `\p{N}`, not `[0-9]`: a roman numeral, a fraction glyph, and a superscript are all figures a
/// reader reads as data, and an ASCII-only check waves all three through.
#[test]
fn non_ascii_numerics_are_rejected_too() {
    let allow = Slot::ExecutiveSummary.allowlist();
    for text in [
        "Spend rose by ½ over the window.",
        "Phase Ⅳ of the migration.",
        "A ² increase.",
    ] {
        let err = validate(text, allow).unwrap_err();
        assert!(matches!(err, Violation::Numeric(_)), "{text:?} gave {err:?}");
    }
}

#[test]
fn a_key_outside_the_slots_allowlist_is_rejected_even_though_it_resolves_globally() {
    let reg = registry_for(&sample_report());
    // The key is real: the registry carries it, and the document layer prints it.
    assert!(reg.get("unit-costs.per-session").is_some() || reg.get("attribution.covered").is_some());
    // It is simply not on THIS slot's allowlist, which is the sharpest hole the design review found.
    let err = validate(
        "Cost ran {{fact:attribution.covered}} for the window.",
        Slot::Closing.allowlist(),
    )
    .unwrap_err();
    assert_eq!(err, Violation::OffAllowlist("attribution.covered".to_string()));
}

#[test]
fn stray_braces_are_rejected() {
    let allow = Slot::ExecutiveSummary.allowlist();
    for text in [
        "Spend was {{totals.spend}}.",
        "Spend was }} high.",
        "Spend was {{ high.",
    ] {
        let err = validate(text, allow).unwrap_err();
        assert!(matches!(err, Violation::StrayBraces), "{text:?} gave {err:?}");
    }
}

/// Rule 6 of the contract ("No em dashes") must be ENFORCED, not merely declared.
///
/// It sat in the system prompt calling itself non-negotiable while `validate` checked only digits,
/// braces, and structure, so a reply carrying an em dash shipped unchecked. Em dashes are a
/// well-known LLM habit and a house-style violation that is hard to spot after publication.
#[test]
fn em_dashes_are_rejected() {
    let allow = Slot::ExecutiveSummary.allowlist();
    for text in [
        "The window ran long \u{2014} and cost more than the last one.",
        "Spend of {{fact:totals.spend}} \u{2014} concentrated in one repo.",
    ] {
        let err = validate(text, allow).unwrap_err();
        assert!(matches!(err, Violation::EmDash), "{text:?} gave {err:?}");
    }
}

/// The forms Scott's house style prescribes INSTEAD of an em dash must still pass, or the check
/// above would just push the model toward a different violation.
#[test]
fn the_em_dash_substitutes_are_accepted() {
    let allow = Slot::ExecutiveSummary.allowlist();
    for text in [
        "The window ran long -- and cost more than the last one.",
        "The window ran long: it cost more than the last one.",
        "The window ran long (and cost more than the last one).",
        "An en dash \u{2013} is not an em dash.",
    ] {
        assert!(validate(text, allow).is_ok(), "{text:?} must be accepted");
    }
}

/// Block-structure injection needs no leading `#`. Each of these parses as a non-paragraph node
/// under the same grammar marquee renders with, and each must be rejected.
#[test]
fn every_block_structure_form_is_rejected() {
    let allow = Slot::ExecutiveSummary.allowlist();
    let cases = [
        ("atx heading", "# Spend\n\nIt was high."),
        ("setext heading", "Spend\n=====\n\nIt was high."),
        ("table", "| Repo | Spend |\n|---|---|\n| a | b |"),
        ("blockquote", "> Spend was high."),
        ("bullet list", "- Spend was high.\n- It was fine."),
        ("fenced code", "```\nspend\n```"),
        ("raw html block", "<div>Spend was high.</div>"),
        ("thematic break", "Spend was high.\n\n---\n\nAnd then it fell."),
    ];
    for (name, text) in cases {
        let err = validate(text, allow).unwrap_err();
        assert!(
            matches!(err, Violation::Structure(_)),
            "{name} was not rejected as structure: {err:?}"
        );
    }
}

/// A plain `#`-check would pass a setext heading and a table. This is the test that proves comrak is
/// load-bearing rather than decorative.
#[test]
fn the_structural_check_catches_what_a_leading_hash_check_would_miss() {
    let allow = Slot::ExecutiveSummary.allowlist();
    for text in ["Spend\n=====\n", "| a | b |\n|---|---|\n| c | d |"] {
        assert!(!text.trim_start().starts_with('#'), "precondition: no leading hash");
        assert!(validate(text, allow).is_err(), "{text:?} slipped through");
    }
}

/// Inline emphasis, code spans, and links are ordinary prose and must NOT be rejected.
#[test]
fn inline_prose_markup_is_allowed() {
    let allow = Slot::ExecutiveSummary.allowlist();
    let text = "Work concentrated in `{{fact:repos.top}}`, which was *by far* the **busiest** repo.";
    assert!(validate(text, allow).is_ok());
}

#[test]
fn an_empty_reply_is_rejected() {
    let allow = Slot::ExecutiveSummary.allowlist();
    assert_eq!(validate("   \n  ", allow).unwrap_err(), Violation::Empty);
    assert_eq!(validate("", allow).unwrap_err(), Violation::Empty);
}

#[test]
fn multiple_paragraphs_are_allowed() {
    let allow = Slot::ExecutiveSummary.allowlist();
    let text = "First paragraph of prose.\n\nSecond paragraph of prose.";
    assert!(validate(text, allow).is_ok());
}

// ---------------------------------------------------------------------------------------------
// The retry ladder and degradation
// ---------------------------------------------------------------------------------------------

/// Phase 2 success criterion (1): a digit-bearing reply retries once, and a second violation ships
/// the slot EMPTY. The artifact is still written -- that is asserted in the render test below.
#[test]
fn a_digit_bearing_reply_retries_once_then_ships_empty() {
    let reg = registry_for(&sample_report());
    let transport = Scripted::always("This period ran 1,527 sessions for $9,450.31.");

    let prose = generate(&transport, &reg, MODEL, CEILING, false);

    assert!(prose.is_empty(), "every slot violated twice, so none ships prose");
    let calls = transport.calls();
    assert_eq!(
        calls.len(),
        UNCONDITIONAL.len() * 2,
        "each slot is attempted exactly twice: {} calls",
        calls.len()
    );
}

/// The retry NAMES the violation. A retry that just says "try again" is the blind re-fire the
/// no-retry doctrine forbids; naming the rule broken is what earns the exemption.
#[test]
fn the_retry_prompt_names_the_specific_violation() {
    let reg = registry_for(&sample_report());
    let transport = Scripted::always("Sessions numbered 1,527 this period.");

    generate(&transport, &reg, MODEL, CEILING, false);

    let calls = transport.calls();
    let first = &calls[0];
    let retry = &calls[1];
    assert!(
        !first.prompt.contains("REJECTED"),
        "the first attempt is the plain prompt"
    );
    assert!(
        retry.prompt.contains("YOUR PREVIOUS ATTEMPT WAS REJECTED"),
        "got: {}",
        retry.prompt
    );
    assert!(
        retry.prompt.contains("numeric character '1'"),
        "the retry must name the character that broke the rule: {}",
        retry.prompt
    );
}

/// A slot that conforms on the SECOND attempt is kept. The retry is not decoration.
#[test]
fn a_slot_that_conforms_on_retry_is_accepted() {
    let reg = registry_for(&sample_report());
    let transport = Scripted::sequence(&["This period ran 1,527 sessions.", GOOD]);

    let prose = generate(&transport, &reg, MODEL, CEILING, false);

    assert_eq!(prose.get("executive-summary").map(String::as_str), Some(GOOD));
}

/// A transport error degrades IMMEDIATELY -- no retry. Retrying a failed subprocess is exactly what
/// the no-retry doctrine forbids; the exemption is scoped to contract violations only.
#[test]
fn a_transport_error_degrades_without_retrying() {
    let reg = registry_for(&sample_report());
    let prose = generate(&Exploding, &reg, MODEL, CEILING, false);
    assert!(prose.is_empty());
}

#[test]
fn tradeoffs_is_generated_only_when_requested() {
    let reg = registry_for(&report_with_efficiency());

    let without = Scripted::always(GOOD);
    generate(&without, &reg, MODEL, CEILING, false);
    assert!(!without.calls().iter().any(|c| c.prompt.contains("tradeoffs")));

    let with = Scripted::always("Cache reuse sat at {{fact:efficiency.cache-read-share}} for the window.");
    generate(&with, &reg, MODEL, CEILING, true);
    assert!(
        with.calls().len() > without.calls().len(),
        "including tradeoffs adds a call"
    );
}

// ---------------------------------------------------------------------------------------------
// The brief
// ---------------------------------------------------------------------------------------------

/// Phase 2 success criterion (2): the payload is the BRIEF only -- small, and free of every
/// context-block field. Feeding the block in would recreate the "940KB of temptation" the guard
/// existed to police.
#[test]
fn the_slot_payload_is_the_brief_only() {
    let reg = registry_for(&report_with_efficiency());
    let transport = Scripted::always(GOOD);
    generate(&transport, &reg, MODEL, CEILING, true);

    for call in transport.calls() {
        assert!(
            call.brief.len() <= MAX_BRIEF_BYTES,
            "brief is {} bytes, over the {MAX_BRIEF_BYTES} cap: {}",
            call.brief.len(),
            call.brief
        );
        // No context-block field name may appear. `sessions`, `summary`, `title`, and `tags` are the
        // user-derived free text a slot must NEVER receive.
        for field in [
            "\"sessions\"",
            "\"summary\"",
            "\"title\"",
            "\"tags\"",
            "\"aggregates\"",
            "\"by-day\"",
            "\"outliers\"",
            "\"persona\"",
            "\"basis\"",
            "\"reconciliation\"",
            "\"enrichment-coverage\"",
            "\"efficiency\"",
        ] {
            assert!(
                !call.brief.contains(field),
                "brief leaked the context-block field {field}: {}",
                call.brief
            );
        }
        assert_eq!(call.kind, Kind::Slot);
        assert_eq!(call.model, MODEL);
        assert_eq!(call.ceiling, CEILING);
        assert!(call.system.contains("HARD CONTRACT"));
    }
}

/// The brief carries ONLY the slot's own allowlisted keys, so a key the slot may not cite is never
/// even shown to it.
#[test]
fn the_brief_carries_only_this_slots_allowlisted_keys() {
    let reg = registry_for(&sample_report());
    let json = brief(Slot::Closing, &reg);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let facts = parsed["facts"].as_object().unwrap();

    for key in facts.keys() {
        assert!(
            Slot::Closing.allowlist().contains(&key.as_str()),
            "brief carried {key}, which is off the Closing allowlist"
        );
    }
    assert!(facts.contains_key("totals.spend"));
    assert!(
        !facts.contains_key("attribution.covered"),
        "an off-allowlist key is not shown"
    );
}

/// The closing brief carries the `late-period.*` facts, and prose citing them validates.
///
/// These keys are what replaced the retired `Forward-Looking` section, so this is the assertion that
/// the replacement is actually reachable: on the allowlist, in the brief, and accepted by the
/// validator. Without it the keys could sit in `facts.rs` forever, registered and never citable.
#[test]
fn the_closing_slot_can_cite_the_late_period_facts() {
    let reg = registry_for(&sample_report());
    let json = brief(Slot::Closing, &reg);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let facts = parsed["facts"].as_object().unwrap();

    for key in [
        "late-period.days",
        "late-period.sessions",
        "late-period.spend",
        "late-period.active-days",
    ] {
        assert!(facts.contains_key(key), "the closing brief omits {key}: {json}");
    }

    let reply = "The period closed as it ran. The final {{fact:late-period.days}} days carried \
                 {{fact:late-period.sessions}} sessions and {{fact:late-period.spend}}.";
    assert!(
        validate(reply, Slot::Closing.allowlist()).is_ok(),
        "closing prose citing the late-period facts must validate"
    );
}

/// The tail sentence must degrade, not fail the render, on a window too short to have a tail.
///
/// `facts.rs` registers no `late-period.*` key under a fortnight, so a model that writes the
/// sentence anyway is REJECTED for citing an unregistered key -- one retry, then an empty slot. That
/// is the intended path, and it is only correct if the key is genuinely off the resolvable set.
#[test]
fn late_period_prose_is_rejected_when_the_window_is_too_short() {
    let mut report = sample_report();
    report.since = ts("2026-04-10T00:00:00Z");
    report.until = ts("2026-04-16T00:00:00Z");
    let reg = registry_for(&report);

    let json = brief(Slot::Closing, &reg);
    assert!(
        !json.contains("late-period"),
        "a short window shows the model no late-period fact: {json}"
    );

    // The key is still ON the allowlist (it is static), so validation passes but interpolation has
    // nothing to substitute. That is the failure the post-interpolation re-check exists to catch.
    let reply = "Closing. The final {{fact:late-period.days}} days carried nothing.";
    let accepted = validate(reply, Slot::Closing.allowlist()).unwrap();
    assert!(
        document::interpolate(&accepted, &reg).is_err(),
        "an unresolvable fact must fail rather than interpolate to empty"
    );
}

/// An allowlisted key the registry does not carry is OMITTED rather than sent as null or "n/a": the
/// model is never shown a fact it cannot truthfully cite.
#[test]
fn an_absent_fact_is_omitted_from_the_brief() {
    let reg = registry_for(&sample_report());
    let json = brief(Slot::Tradeoffs, &reg);
    assert!(
        !json.contains("n/a"),
        "the not-measured sentinel never reaches a brief: {json}"
    );
    assert!(!json.contains("null"), "an absent fact is omitted, not nulled: {json}");
}

#[test]
fn every_allowlisted_key_is_a_real_registry_key_for_a_rich_report() {
    let reg = registry_for(&report_with_efficiency());
    // Not every key resolves for every report (an absent ratio, no prior period), but a key that
    // resolves for NO report is a typo that would silently shrink a brief forever.
    let slots = [
        Slot::ExecutiveSummary,
        Slot::WhatThisFunded,
        Slot::UsageProfile,
        Slot::Closing,
        Slot::Tradeoffs,
    ];
    for slot in slots {
        let resolved = slot.allowlist().iter().filter(|k| reg.get(k).is_some()).count();
        assert!(
            resolved > 0,
            "no key on {}'s allowlist resolves at all: {:?}",
            slot.key(),
            slot.allowlist()
        );
    }
}

#[test]
fn slot_keys_are_unique_and_kebab_case() {
    let slots = [
        Slot::ExecutiveSummary,
        Slot::WhatThisFunded,
        Slot::UsageProfile,
        Slot::Closing,
        Slot::Tradeoffs,
    ];
    let mut keys: Vec<&str> = slots.iter().map(|s| s.key()).collect();
    let count = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), count, "slot keys collide");
    for key in keys {
        assert!(
            key.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "slot key {key:?} is not kebab-case"
        );
    }
}

/// Every slot's prompt must actually be present and non-trivial: an `include_str!` of an empty file
/// compiles fine and produces an unusable call.
#[test]
fn every_slot_carries_a_real_prompt() {
    for slot in [
        Slot::ExecutiveSummary,
        Slot::WhatThisFunded,
        Slot::UsageProfile,
        Slot::Closing,
        Slot::Tradeoffs,
    ] {
        let prompt = slot.prompt();
        assert!(prompt.len() > 100, "{} has a stub prompt", slot.key());
        assert!(prompt.contains("Intent:"), "{} states no intent", slot.key());
        assert!(prompt.contains(slot.key()), "{} does not name itself", slot.key());
        assert!(!prompt.contains('—'), "{} carries an em dash", slot.key());
    }
}

// ---------------------------------------------------------------------------------------------
// The post-interpolation re-check
// ---------------------------------------------------------------------------------------------

#[test]
fn verify_interpolated_accepts_substituted_prose() {
    assert!(verify_interpolated("This period ran 1,527 sessions for $9,450.31.").is_ok());
}

/// After interpolation the DIGIT rule no longer applies (that is the point of interpolation), but
/// the structural and brace rules do.
#[test]
fn verify_interpolated_still_rejects_structure_and_braces() {
    assert!(verify_interpolated("# 1,527 sessions").is_err());
    assert!(verify_interpolated("| a | b |\n|---|---|\n| c | d |").is_err());
    assert!(verify_interpolated("Spend was {{fact:nope}}.").is_err());
}

// ---------------------------------------------------------------------------------------------
// End to end: no slot failure can cost the artifact
// ---------------------------------------------------------------------------------------------

/// Render the document with the prose a given transport produced.
fn render_via(report: &Report, transport: &impl Transport, include_tradeoffs: bool) -> String {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let opts = ViewOpts {
        include_tradeoffs,
        ..ViewOpts::default()
    };
    let block = document::build_views(report, &aggregates, &persona, &pricing, opts).unwrap();
    let reg = document::registry(&block);
    let prose = generate(transport, &reg, MODEL, CEILING, include_tradeoffs);
    document::render(&block, &prose, document::ChartMode::Svg).markdown
}

/// Phase 2 success criterion (1), the half that matters most: a slot that violates twice degrades to
/// an empty section and the ARTIFACT IS STILL WRITTEN, complete. No code path can discard it.
#[test]
fn a_slot_violating_twice_costs_a_paragraph_and_nothing_else() {
    let report = report_with_efficiency();
    let poisoned = render_via(
        &report,
        &Scripted::always("Sessions numbered 1,527 for $9,450.31."),
        true,
    );

    // The prose sections are present but empty...
    assert!(poisoned.contains("## Executive Summary"));
    assert!(poisoned.contains("## Conclusion"));
    // ...and not one fabricated figure reached the artifact.
    assert!(!poisoned.contains("1,527"), "the rejected reply's digits never land");
    assert!(!poisoned.contains("9,450.31"));
    // ...while every data section rendered in full.
    for section in [
        "## Cost Summary",
        "## Reconciliation",
        "## The Efficiency Story",
        "## What This Funded",
        "## Usage Profile",
    ] {
        assert!(poisoned.contains(section), "missing {section}");
    }
    assert!(poisoned.len() > 1_000, "the artifact is complete, not a stub");
}

/// The same render with a transport that cannot be reached at all: identical outcome. This is the
/// offline story, and it is a degradation rather than an error.
#[test]
fn an_unreachable_transport_produces_the_same_complete_artifact() {
    let report = report_with_efficiency();
    let exploded = render_via(&report, &Exploding, false);
    let refused = render_via(&report, &Scripted::always("nope 1"), false);

    assert_eq!(
        exploded, refused,
        "a dead transport and a non-conforming one degrade to the same artifact"
    );
    assert!(exploded.contains("## Cost Summary"));
}

/// A conforming slot's prose reaches the artifact with its figures substituted -- and those figures
/// are the SAME strings the document's own tables print.
#[test]
fn conforming_slot_prose_lands_with_the_documents_own_figures() {
    let report = sample_report();
    let md = render_via(&report, &Scripted::always(GOOD), false);

    assert!(
        md.contains("Across 30 days this account ran 2 sessions for $0.60"),
        "got:\n{md}"
    );
    assert!(!md.contains("{{fact:"), "no placeholder survives");
    // $0.60 is what the Cost Summary total row prints, so prose and table cannot disagree.
    assert!(md.contains("| **Total** | 2 |"));
}

#[test]
fn node_name_is_readable() {
    assert_eq!(node_name(&NodeValue::Paragraph), "paragraph");
    assert_eq!(node_name(&NodeValue::ThematicBreak), "thematicbreak");
}
