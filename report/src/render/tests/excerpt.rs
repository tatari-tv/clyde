#![allow(clippy::unwrap_used)]

//! Phase 1: `excerpt_at` quotes the SPAN the guard actually rejected, not a fresh substring search
//! over the whole document. The prior `excerpt(prose, needle)` re-scanned `prose` for `needle`'s
//! first `starts_with` match with no word-boundary check and no connection to the offset the guard's
//! own regex found, which is how `500` quoted a line carrying the licensed `$1,500.08` and `100`
//! quoted an unrelated model id (`design 2026-07-27-month-over-month-deltas.md`, defect 3).

use super::*;
use crate::quotable::QuotableFacts;

/// THE phase criterion. `500` sits first inside a licensed comma-grouped figure (`$1,500.08`, a real
/// display figure the facts license as `1500.08` -- an entirely different token from a bare `500`)
/// and again as the real fabricated figure later in the same prose. `excerpt_at` must land on the
/// SECOND occurrence, because that is the span `foreign_figures` actually flagged; the first one was
/// never a match for the bare token `500` at all.
///
/// BITES against the prior `excerpt`: a whole-document `starts_with` search for `"500"` finds it
/// inside `"1,500.08"` first and quotes the wrong sentence.
#[test]
fn excerpt_quotes_the_rejected_span_not_an_earlier_lookalike() {
    let facts = QuotableFacts::from_context_json(r#"{"totals":{"spend":"$1,500.08"}}"#).unwrap();
    // A generous gap between the two occurrences (well past `EXCERPT_RADIUS`'s 60 chars either
    // side), so a correct excerpt around the SECOND span cannot also reach back to the first.
    let prose = "Total spend for the window was $1,500.08 across every session tracked in this \
                 period, attributed cleanly with no unmodeled remainder to speak of anywhere in \
                 the ledger. Separately, and unprompted, the model went on to say it selected the \
                 top 500 sessions by cost for its own ranking.";

    let foreign = facts.foreign_figures(prose);
    assert_eq!(
        foreign.len(),
        1,
        "the licensed $1,500.08 must not itself be foreign: {foreign:?}"
    );
    let figure = &foreign[0];
    assert_eq!(
        figure.token, "500",
        "the fabricated figure is the bare 500, not the licensed 1500.08"
    );

    let excerpt = excerpt_at(prose, figure.start, figure.end);
    assert!(
        excerpt.contains("top 500 sessions"),
        "the excerpt must quote the actual violating clause, not the earlier lookalike: {excerpt:?}"
    );
    assert!(
        !excerpt.contains("1,500.08"),
        "the excerpt must not land on the licensed comma-grouped figure: {excerpt:?}"
    );
}

/// The prior `excerpt` matched against the NORMALIZED token (commas stripped), so a comma-grouped
/// fabricated figure could never be found verbatim in `prose` and the excerpt came back empty --
/// exactly when the operator needed it most, on the harder-to-read figure. `excerpt_at` takes the
/// span the regex matched in the ORIGINAL text, commas included, so this must never be empty.
#[test]
fn excerpt_of_a_comma_grouped_token_is_not_empty() {
    let facts = QuotableFacts::from_context_json(r#"{"totals":{"spend":"$4.12"}}"#).unwrap();
    let prose = "The window invented a total of 9,450.31 dollars in spend.";

    let foreign = facts.foreign_figures(prose);
    assert_eq!(foreign.len(), 1);
    let figure = &foreign[0];

    let excerpt = excerpt_at(prose, figure.start, figure.end);
    assert!(
        !excerpt.is_empty(),
        "a comma-grouped fabricated figure must still produce an excerpt"
    );
    assert!(
        excerpt.contains("9,450.31"),
        "the excerpt must carry the original comma-grouped text: {excerpt:?}"
    );
}

/// Mandatory per the design doc: this crate has already shipped the byte/char conflation bug once
/// (`eval/mechanical.rs`'s `em_dash`, documented postmortem in its own comment). `start`/`end` arrive
/// as BYTE offsets from the regex; multibyte characters ahead of the match must not slide the
/// char-based radius window. Three emoji (4 bytes each) sit before the fabricated figure so a
/// byte/char conflation would either panic (indexing a `Vec<char>` past its length) or land the
/// window on the wrong text.
#[test]
fn excerpt_lands_on_the_right_span_with_multibyte_text_before_it() {
    let facts = QuotableFacts::from_context_json(r#"{"totals":{"spend":"$4.12"}}"#).unwrap();
    let prose = "\u{1F389}\u{1F389}\u{1F389} the window reached 42 sessions this month.";

    let foreign = facts.foreign_figures(prose);
    assert_eq!(foreign.len(), 1, "{foreign:?}");
    let figure = &foreign[0];
    assert_eq!(figure.token, "42");

    let excerpt = excerpt_at(prose, figure.start, figure.end);
    assert!(
        excerpt.contains("reached 42 sessions"),
        "the excerpt must land on the actual clause despite the multibyte prefix: {excerpt:?}"
    );
}
