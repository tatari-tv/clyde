#![allow(clippy::unwrap_used)]

//! The `notes` context field (design "API Design"): `Report.notes` is populated on every v2
//! artifact and used to stop at the artifact -- these pin that it now reaches the prompt, that an
//! empty list emits no key, that a note's digits stay unquotable outside the sentence itself, and
//! that both templates document it.

use super::*;

/// Design "API Design", the `notes` context field: `Report.notes` exists on every v2 artifact and
/// never reached the prompt, so the M2 window statement and every merge caveat were invisible to the
/// reader. A report WITH notes surfaces them verbatim, one entry per note.
///
/// BITES: drop `notes` from `ContextBlock` (or from `build_context_block`) and the `expect` fails;
/// reformat a note on the way through and the verbatim assertion catches it.
#[test]
fn build_context_block_surfaces_the_report_notes() {
    let mut report = sample_report();
    report.notes = vec![
        crate::report::WINDOW_NOTE.to_string(),
        "merged: `cache` omitted, one input lacked it".to_string(),
    ];
    let block = ctx(&report, false);
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let notes = parsed
        .get("notes")
        .and_then(|v| v.as_array())
        .expect("notes key must be present when the report carries notes");
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].as_str(), Some(crate::report::WINDOW_NOTE));
    assert_eq!(
        notes[1].as_str(),
        Some("merged: `cache` omitted, one input lacked it"),
        "each note reaches the prompt verbatim, never reformatted"
    );
}

/// The absent case: no notes means NO KEY, not an empty list, so the prompt's "write no such note"
/// rule needs no empty-vs-absent special case.
#[test]
fn context_block_omits_notes_entirely_when_the_report_has_none() {
    let report = sample_report();
    assert!(report.notes.is_empty());
    let parsed: serde_json::Value = serde_json::from_str(&ctx(&report, false)).unwrap();
    assert!(
        parsed.get("notes").is_none(),
        "an empty notes list must serialize to no key at all"
    );
}

/// A note is free text, so it is an IDENTIFIER (quotable whole) and never a figure source: quoting
/// the sentence verbatim passes the prose guard, and a bare number lifted out of it does not.
///
/// BITES: drop `notes` from `quotable::IDENTIFIER_KEYS` and the note's digits become licensed
/// figures, so the second assertion (the lifted `8675309`) stops being rejected.
#[test]
fn a_notes_digits_are_quotable_only_inside_the_verbatim_sentence() {
    let note = "merged: `cache` omitted; 8675309 records lacked it";
    let mut report = sample_report();
    report.notes = vec![note.to_string()];
    let facts = build_context_block(
        &report,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        None,
        None,
    )
    .unwrap()
    .facts;
    assert!(
        facts.foreign_figures(&format!("Methodology: {note}.")).is_empty(),
        "the note must be quotable verbatim"
    );
    let foreign = facts.foreign_figures("The window covered 8675309 sessions.");
    assert_eq!(
        foreign.iter().map(|f| f.token.clone()).collect::<Vec<_>>(),
        vec!["8675309".to_string()],
        "a number lifted out of a note is not a licensed figure"
    );
}

/// The `notes` prompt-edit ledger: BOTH templates document the field, both require it verbatim, and
/// both state the absent case, or the two formats drift on the one field that explains how the
/// artifact was produced.
#[test]
fn both_templates_document_the_notes_field() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            tpl.contains("- `notes`: OPTIONAL"),
            "{name} must document the notes field as optional"
        );
        assert!(
            tpl.contains("verbatim (never paraphrase, never restate its numbers)"),
            "{name} must require each note verbatim"
        );
        assert!(
            tpl.contains("Methodology"),
            "{name} must name where the notes land in the artifact"
        );
    }
}
