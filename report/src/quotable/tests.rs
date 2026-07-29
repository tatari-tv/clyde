#![allow(clippy::unwrap_used)]

use super::*;

/// A context block shaped like the real one: display figures the prose may quote, an untitled
/// session cited only by `short-id`, a commit sha, a PR ref, and free-text `title`/`summary`. Every
/// identifier here carries digits that must NOT become quotable figures -- `a14bc3d2` and the sha
/// both contain `14`, which is exactly how the pre-change whitelist pre-approved a fabricated
/// "14 hours".
fn context() -> &'static str {
    r#"{
      "period": {"since": "2026-07-01", "until": "2026-07-30", "days": 30, "active-days": 22},
      "totals": {"sessions": 118, "spend": "$9,450.31", "tokens-human": "1.2B"},
      "unit-costs": {"per-commit": "$37.20", "per-pr": "$412.19"},
      "efficiency": {"cache-read-share": "96.0%", "by-skill-coverage": "$412.19 of $9,450.31 (4.4%)"},
      "aggregates": {
        "by-day": [{"date": "2026-07-01", "sessions": 6, "spend": "$212.55", "active": true,
                    "spend-percent-of-max": 63.5}],
        "by-repo": [{"repo": "tatari-tv/clyde", "sessions": 41, "spend": "$3,120.08",
                     "repo-source": "git-origin",
                     "outcomes": {"commits": 88, "prs-opened": 9}}]
      },
      "sessions": [
        {"short-id": "a14bc3d2", "title": null, "repo": "tatari-tv/clyde",
         "begin": "2026-07-03T09:14:22Z", "end": "2026-07-03T11:02:41Z",
         "tokens-human": "48.2M", "spend-display": "$61.40",
         "models": ["claude-opus-4-7"],
         "outcomes": {"commits": ["8f14e45fceea167a5a36dedd4bea2543"],
                      "prs": [{"number": 62, "url": "https://github.com/tatari-tv/clyde/pull/62",
                               "repository": "tatari-tv/clyde"}]}},
        {"short-id": "7b2290ff", "title": "narrow the number guard",
         "summary": "Rewrote the guard so 14 whitelisted tokens stopped being pre-approved.",
         "tags": ["render", "guard"],
         "begin": "2026-07-11T13:00:00Z", "end": "2026-07-11T15:30:00Z",
         "tokens-human": "12.7M", "spend-display": "$18.05", "models": ["claude-opus-4-7"]}
      ]
    }"#
}

fn facts() -> QuotableFacts {
    QuotableFacts::from_context_json(context()).unwrap()
}

/// The rejected tokens only, in the order `foreign_figures` found them (match order, not sorted).
/// Most assertions in this file care WHICH numbers were rejected, not the exact span; the span
/// itself is exercised by the excerpt tests in `render/tests/excerpt.rs`.
fn tokens(foreign: &[ForeignFigure]) -> Vec<String> {
    foreign.iter().map(|f| f.token.clone()).collect()
}

/// THE phase criterion. A fabricated "14 hours of engineering time" is rejected, and the assertion
/// that makes the test bite: the PRE-CHANGE whitelist contains `14` (it falls inside the `a14bc3d2`
/// short-id and the commit sha), so the old guard passed this exact sentence. Revert the guard to
/// `all_numeric_tokens` and the first assertion fails.
#[test]
fn planted_hours_claim_is_rejected_where_the_pre_change_whitelist_passed() {
    let facts = facts();
    let planted = "The window saved roughly 14 hours of engineering time.";

    let foreign = facts.foreign_figures(planted);
    assert_eq!(
        tokens(&foreign),
        vec!["14".to_string()],
        "the planted figure must be named"
    );

    assert!(
        all_numeric_tokens(context()).contains("14"),
        "the pre-change whitelist pre-approved 14 from an identifier, which is why this phase exists"
    );
}

/// A "3x" style multiplier the model invents is caught by the same narrowing: `3` is in the block
/// only inside identifiers and a decimal's fractional part, never as a standalone figure.
#[test]
fn fabricated_multiplier_is_rejected() {
    let foreign = facts().foreign_figures("Throughput ran 3x the prior period.");
    assert_eq!(tokens(&foreign), vec!["3".to_string()]);
}

/// False-positive case 1: an UNTITLED session cited by `short-id`. The id's digits are exempt
/// inside the citation, and the session's own display figures still pass.
#[test]
fn untitled_session_cited_by_short_id_passes() {
    let prose = "The largest untitled session, a14bc3d2, spent $61.40 across 48.2M tokens \
                 between 2026-07-03T09:14:22Z and 2026-07-03T11:02:41Z.";
    assert!(
        facts().foreign_figures(prose).is_empty(),
        "citing an untitled session by short-id must not read as fabrication"
    );
}

/// False-positive case 2: a prose PR reference, in the `#62`, bare-`62` and full-url forms a
/// narrative actually uses.
#[test]
fn prose_pr_reference_passes_in_every_cited_form() {
    let facts = facts();
    for prose in [
        "That work landed in #62.",
        "That work landed in PR 62.",
        "That work landed in https://github.com/tatari-tv/clyde/pull/62.",
    ] {
        assert!(
            facts.foreign_figures(prose).is_empty(),
            "a legitimate PR citation must pass: {prose}"
        );
    }
}

/// A commit sha citation passes in both its full and abbreviated forms, and a date lifted from a
/// session's `begin` passes as a date.
#[test]
fn commit_sha_and_session_date_citations_pass() {
    let facts = facts();
    assert!(
        facts
            .foreign_figures("Fixed in 8f14e45fceea167a5a36dedd4bea2543.")
            .is_empty()
    );
    assert!(facts.foreign_figures("Fixed in 8f14e45.").is_empty());
    assert!(
        facts
            .foreign_figures("The run on 2026-07-03 was the busiest.")
            .is_empty()
    );
}

/// The whole-token rule: a cited identifier only exempts a number it covers ENTIRELY. `#62` masks
/// two bytes, so a fabricated `624` sitting next to it is still caught -- otherwise every cited
/// identifier would silently license every number containing its digits.
#[test]
fn a_partial_identifier_overlap_does_not_exempt_a_longer_number() {
    let foreign = facts().foreign_figures("PR 62 shipped 624 files.");
    assert_eq!(tokens(&foreign), vec!["624".to_string()]);
}

/// Free text is citable but never quotable as a figure: the `14` inside a session `summary` does
/// not license `14` in a headline, though quoting the summary verbatim is fine.
#[test]
fn a_number_inside_free_text_is_citable_but_not_a_figure() {
    let facts = facts();
    assert!(
        facts
            .foreign_figures("One session \"Rewrote the guard so 14 whitelisted tokens stopped being pre-approved.\"")
            .is_empty(),
        "a verbatim summary citation passes"
    );
    assert_eq!(
        tokens(&facts.foreign_figures("The team reclaimed 14 whitelisted tokens.")),
        vec!["14".to_string()],
        "the same digits, restated as the artifact's own figure, do not"
    );
}

/// Display figures the binary computed -- dollars, `tokens-human`, percents, counts, dates, the
/// coverage and unit-cost strings the later phases added -- all pass.
#[test]
fn every_computed_display_figure_passes() {
    let facts = facts();
    let prose = "Across 30 days (22 active) 118 sessions cost $9,450.31 for 1.2B tokens, \
                 $37.20 per commit and $412.19 per PR, at a 96.0% cache-read share \
                 ($412.19 of $9,450.31 (4.4%) covered by skill), 2026-07-01 to 2026-07-30. \
                 tatari-tv/clyde alone ran 41 sessions, $3,120.08, 88 commits and 9 PRs.";
    assert!(
        facts.foreign_figures(prose).is_empty(),
        "computed figures must pass: {:?}",
        facts.foreign_figures(prose)
    );
}

/// The three sets stay separate. A `-percent-of-max` bar width is both a figure and geometry, but a
/// Phase 11 `points` string is geometry ONLY -- its dozens of small integers never reach the prose
/// whitelist.
///
/// Geometry is held as WHOLE values (Phase 11): the guard over it asks "is this attribute value one
/// the binary computed", so a token-level set would license a fabricated `cx="17"` off one point's
/// y coordinate. Phase 10 shipped the tokenized form because nothing consumed it yet.
#[test]
fn geometry_is_kept_out_of_the_prose_whitelist() {
    let facts = QuotableFacts::from_context_json(
        r#"{"charts":{"points":"0,17 1,42 2,88","viewbox":"0 0 640 240"},"row":{"spend-percent-of-max":63.5}}"#,
    )
    .unwrap();

    assert!(facts.licenses_geometry("0,17 1,42 2,88") && facts.licenses_geometry("0 0 640 240"));
    assert!(
        !facts.licenses_geometry("17") && !facts.licenses_geometry("640"),
        "one coordinate out of a points list is not a licensed attribute value: {:?}",
        facts.geometry
    );
    assert!(
        !facts.figures.contains("17") && !facts.figures.contains("640"),
        "a points/viewBox integer is not a quotable prose figure: {:?}",
        facts.figures
    );
    assert!(
        facts.figures.contains("63.5") && facts.licenses_geometry("63.5"),
        "a bar proportion is BOTH a quotable percent and legitimate geometry"
    );
    assert_eq!(
        tokens(&facts.foreign_figures("The chart peaked at 88.")),
        vec!["88".to_string()]
    );
}

/// No blanket small-integer exemption: `0..=100` is NOT seeded, so an unlicensed small integer is
/// caught rather than waved through.
#[test]
fn small_integers_are_not_blanket_exempt() {
    let facts = QuotableFacts::from_context_json(r#"{"totals":{"spend":"$4.12"}}"#).unwrap();
    let foreign = facts.foreign_figures("Roughly 7 engineers, 14 hours, 99 sessions.");
    assert_eq!(
        tokens(&foreign),
        vec!["7".to_string(), "14".to_string(), "99".to_string()],
        "every unlicensed small integer is reported, in the order it was found"
    );
}

/// The narrowing, stated as what LEFT the whitelist: every token the pre-change guard picked up
/// from an identifier is gone from the figure set, while the display figures stay. (The size ratio
/// itself is measured at real scale in `render::tests::quotable`.)
#[test]
fn identifier_tokens_left_the_whitelist_and_display_figures_stayed() {
    let facts = facts();
    let pre_change = all_numeric_tokens(context());

    for from_identifier in ["14", "3", "8", "45"] {
        assert!(
            pre_change.contains(from_identifier),
            "{from_identifier} was pre-approved before, out of a short-id, sha or summary"
        );
        assert!(
            !facts.figures.contains(from_identifier),
            "{from_identifier} must no longer be a quotable figure"
        );
    }
    for display in ["9450.31", "412.19", "96.0", "118", "30", "2026-07-01"] {
        assert!(
            facts.figures.contains(display),
            "the display figure {display} must stay quotable: {:?}",
            facts.figures
        );
    }
    assert!(
        facts.figure_count() < pre_change.len(),
        "the figure set is strictly narrower than the pre-change whitelist"
    );
}

/// `date_prefix` only fires on a real date shape, so a non-date identifier never contributes a
/// bogus ten-char "date" to the citable set.
#[test]
fn date_prefix_requires_a_date_shape() {
    assert_eq!(date_prefix("2026-07-03T09:14:22Z").as_deref(), Some("2026-07-03"));
    assert_eq!(date_prefix("abcd-ef-ghij"), None);
    assert_eq!(date_prefix("2026-07"), None, "too short to be a date");
}

/// An identifier past the cap is skipped rather than scanned on every render; it is the only leaf
/// class that can run long, and a citation nobody types back buys nothing.
#[test]
fn an_oversized_identifier_is_skipped() {
    let long = "x".repeat(MAX_IDENTIFIER_BYTES + 1);
    let json = format!(r#"{{"sessions":[{{"summary":"{long}"}}]}}"#);
    let facts = QuotableFacts::from_context_json(&json).unwrap();
    assert!(facts.identifiers.is_empty(), "the oversized summary is not kept");
}

/// Classification is by leaf key, and an array element inherits its array's key -- which is what
/// makes `commits[]` and `tags[]` identifiers rather than figures.
#[test]
fn array_elements_inherit_their_arrays_key() {
    let facts = QuotableFacts::from_context_json(r#"{"outcomes":{"commits":["9911223344556677"]}}"#).unwrap();
    assert!(facts.identifiers.contains("9911223344556677"));
    assert!(
        facts.figures.is_empty(),
        "a sha's digits are not figures: {:?}",
        facts.figures
    );
}

/// A percentile field is WRITTEN as an ordinal, so the ordinal spelling is licensed and a bare
/// number is not. Found by the Phase 13 render eval, on the first live render it measured: the
/// model wrote "the 90th percentile" about the real `session-spend-p90` figure and the guard
/// rejected a correct sentence as a fabrication, on both render paths.
///
/// BITES: delete `percentile_ordinal` and the first assertion fails; license the bare digits
/// instead of the ordinal and the second one does.
#[test]
fn a_percentile_label_licenses_its_ordinal_spelling_and_not_a_bare_number() {
    let facts =
        QuotableFacts::from_context_json(r#"{"unit-costs":{"session-spend-p50":"$7.29","session-spend-p90":"$8.79"}}"#)
            .unwrap();
    assert_eq!(
        tokens(&facts.foreign_figures("The median was $7.29 and the 90th percentile $8.79.")),
        Vec::<String>::new()
    );
    assert_eq!(
        tokens(&facts.foreign_figures("The window ran for 90 days.")),
        vec!["90".to_string()],
        "a bare 90 is still unlicensed; only the ordinal SPELLING of the label is"
    );
}

/// The ordinal suffix is the English one, teens included -- `p11` is written "11th", never "11st".
#[test]
fn the_percentile_ordinal_uses_the_english_suffix() {
    assert_eq!(percentile_ordinal("p90").as_deref(), Some("90th"));
    assert_eq!(percentile_ordinal("p1").as_deref(), Some("1st"));
    assert_eq!(percentile_ordinal("p2").as_deref(), Some("2nd"));
    assert_eq!(percentile_ordinal("p3").as_deref(), Some("3rd"));
    assert_eq!(percentile_ordinal("p11").as_deref(), Some("11th"));
    assert_eq!(percentile_ordinal("p13").as_deref(), Some("13th"));
    assert_eq!(percentile_ordinal("1h").as_deref(), None);
    assert_eq!(percentile_ordinal("cache").as_deref(), None);
}

/// A context block whose repo slug carries a digit -- the shape that reproduced the module's own
/// motivating bug through a repo name.
fn context_with_digit_bearing_repo() -> &'static str {
    r#"{
      "totals": {"sessions": 118, "spend": "$9,450.31"},
      "aggregates": {
        "by-repo": [{"repo": "tatari-tv/service14", "sessions": 41, "spend": "$3,120.08",
                     "outcomes": {"commits": 88}}]
      },
      "sessions": [
        {"short-id": "7b2290ff", "repo": "tatari-tv/service14", "spend-display": "$61.40"}
      ]
    }"#
}

/// `repo` was missing from `IDENTIFIER_KEYS` while `repository` was present, so a repo slug fell
/// through to `Class::Figure` and `add_figure_tokens` decomposed it: `tatari-tv/service14` licensed
/// a bare `14` as an unconditional prose figure anywhere in the artifact.
#[test]
fn a_digit_in_a_repo_slug_is_not_a_licensed_figure() {
    let facts = QuotableFacts::from_context_json(context_with_digit_bearing_repo()).unwrap();

    let foreign = facts.foreign_figures("The window saved roughly 14 hours of engineering time.");
    assert_eq!(
        tokens(&foreign),
        vec!["14".to_string()],
        "a digit run inside a repo slug must not license a bare figure elsewhere in the prose"
    );
    assert!(
        !facts.figures.contains("14"),
        "the slug's digits belong to the position-masked identifier set, never the free figures set"
    );

    // The slug itself is still citable verbatim -- that is what makes it an identifier.
    assert!(
        facts
            .foreign_figures("Most of it landed in tatari-tv/service14.")
            .is_empty(),
        "citing the repo by name must pass"
    );
}

/// `commits` is overloaded: an array of SHA strings under `sessions[]`, a bare count under
/// `outcomes.totals` / `by-repo[]`. Classifying on the key alone routed the count through
/// `add_identifier`, licensing its digits by verbatim substring rather than as the figure it is.
#[test]
fn a_commit_count_is_a_figure_and_a_commit_sha_is_an_identifier() {
    let facts = facts();

    // `by-repo[].outcomes.commits` is the number 88: a real, quotable display figure.
    assert!(
        facts.figures.contains("88"),
        "a numeric commit COUNT belongs in the figures set: {:?}",
        facts.figures
    );
    assert!(
        facts.foreign_figures("That repo landed 88 commits.").is_empty(),
        "the prose may state the commit count outright"
    );

    // `sessions[].outcomes.commits` is a sha STRING: identifier, cited verbatim and abbreviated.
    assert!(
        facts.identifiers.contains("8f14e45fceea167a5a36dedd4bea2543"),
        "a sha string stays an identifier"
    );
    assert!(
        facts.identifiers.contains("8f14e45"),
        "and keeps its short-prefix citation form"
    );
    assert!(
        !facts.figures.contains("8f14e45fceea167a5a36dedd4bea2543"),
        "a sha is never decomposed into figures"
    );
}

#[test]
fn classify_reads_the_shape_only_for_the_overloaded_key() {
    // The overloaded key swings on shape ...
    assert_eq!(classify("commits", Shape::Text), Class::Identifier);
    assert_eq!(classify("commits", Shape::Number), Class::Figure);
    // ... and nothing else does.
    assert_eq!(classify("repo", Shape::Text), Class::Identifier);
    assert_eq!(classify("repo", Shape::Number), Class::Identifier);
    assert_eq!(classify("spend", Shape::Text), Class::Figure);
    assert_eq!(classify("spend", Shape::Number), Class::Figure);
    assert_eq!(classify("points", Shape::Text), Class::Geometry);
    assert_eq!(
        classify("spend-percent-of-max", Shape::Number),
        Class::FigureAndGeometry
    );
}

/// A context whose free text carries the shapes the 2026-07-28 live failures rejected: dotted
/// versions and a 3-digit status code inside an enrich summary.
fn versioned_context() -> QuotableFacts {
    QuotableFacts::from_context_json(
        r#"{
          "totals": {"sessions": 12, "spend": "$100.00"},
          "sessions": [
            {"short-id": "9d4c1f28", "title": "cut the release",
             "summary": "bump to v0.5.4 after the API 500 retry storm; v0.5.0 shipped Friday",
             "tags": ["release"],
             "begin": "2026-07-03T09:00:00Z", "end": "2026-07-03T10:00:00Z"}
          ]
        }"#,
    )
    .unwrap()
}

/// BITES: the exact live failure class of 2026-07-28. Three renders in a row were rejected for
/// paraphrasing TRUE numbers out of commit-message text ("shipped as v0.5.0" against a summary
/// reading "bump to v0.5.0"), because the mask demanded the whole identifier verbatim. Drop the
/// `cited` set (or re-split versions into `0.5` + `4`) and this fails.
#[test]
fn paraphrased_versions_and_status_codes_from_summaries_are_licensed() {
    let facts = versioned_context();
    let prose = "The team shipped v0.5.4 once the 500 errors stopped; v0.5.0 had landed earlier.";
    assert_eq!(tokens(&facts.foreign_figures(prose)), Vec::<String>::new());
}

/// The boundary the licensing rule preserves: a bare 1-2 digit integer from free text is NOT
/// licensed on its own -- the fabricated "14 hours" stays caught even when a summary carries a
/// bare `14` -- but quoting the source sentence verbatim still passes via the mask.
#[test]
fn bare_small_integers_from_free_text_stay_verbatim_only() {
    let facts = QuotableFacts::from_context_json(
        r#"{"sessions": [{"short-id": "7b2290ff", "title": "flake hunt",
             "summary": "fixed 14 flaky tests in the permit crate",
             "begin": "2026-07-11T13:00:00Z", "end": "2026-07-11T15:30:00Z"}]}"#,
    )
    .unwrap();
    let paraphrase = "The window saved roughly 14 hours of engineering time.";
    assert_eq!(tokens(&facts.foreign_figures(paraphrase)), vec!["14"]);
    let quoted = r#"One session "fixed 14 flaky tests in the permit crate" and moved on."#;
    assert_eq!(tokens(&facts.foreign_figures(quoted)), Vec::<String>::new());
}

/// A dotted version lexes as ONE token and its `v` prefix is canonicalized away: citing `v0.5.4`
/// licenses `v0.5.4` AND `0.5.4`, and never a standalone `4`. BITES: the 2026-07-28 measurement
/// found 6 of 7 live rejections were prose writing the conventional `v0.6.5` against a summary
/// carrying the bare `0.6.5`; make the prefix significant again and this fails.
#[test]
fn a_version_licenses_both_prefixed_and_bare_forms_but_no_bare_digit() {
    let facts = versioned_context();
    assert_eq!(tokens(&facts.foreign_figures("We made 4 attempts.")), vec!["4"]);
    assert_eq!(
        tokens(&facts.foreign_figures("We are on v0.5.4 now.")),
        Vec::<String>::new()
    );
    assert_eq!(
        tokens(&facts.foreign_figures("We are on 0.5.4 now.")),
        Vec::<String>::new()
    );

    // The live failure shape, exactly: bare source form, conventional v-form in prose.
    let bare_source = QuotableFacts::from_context_json(
        r#"{"sessions": [{"short-id": "1a2b3c4d", "title": "release review",
             "summary": "reviewed a workspace version bump in Cargo.toml from 0.6.4 to 0.6.5",
             "begin": "2026-07-20T09:00:00Z", "end": "2026-07-20T10:00:00Z"}]}"#,
    )
    .unwrap();
    assert_eq!(
        tokens(&bare_source.foreign_figures("The bump shipped as v0.6.5 (from v0.6.4).")),
        Vec::<String>::new()
    );
    // A WRONG version is still foreign in every spelling.
    assert_eq!(
        tokens(&bare_source.foreign_figures("The bump shipped as v0.6.6.")),
        vec!["0.6.6"]
    );
}

/// Random-character identifiers (shas, short-ids, urls) never feed the cited set: a 3+ digit run
/// inside hex must not become a quotable figure, or the pre-change whitelist is back.
#[test]
fn digit_runs_inside_ids_and_shas_are_not_licensed() {
    // `7b2290ff` carries the run `2290`; `versioned_context`'s summary carries no `2290`.
    let facts = QuotableFacts::from_context_json(
        r#"{"sessions": [{"short-id": "7b2290ff", "title": "untitled work",
             "begin": "2026-07-11T13:00:00Z", "end": "2026-07-11T15:30:00Z"}]}"#,
    )
    .unwrap();
    assert_eq!(
        tokens(&facts.foreign_figures("The org ran 2,290 sessions this month.")),
        vec!["2290"]
    );
}

/// A number sourced from nowhere still fails: the cited set widens citation, never invention.
#[test]
fn unsourced_numbers_still_reject_with_the_cited_set_in_play() {
    let facts = versioned_context();
    assert_eq!(
        tokens(&facts.foreign_figures("That saved $777.77 across 999 sessions.")),
        vec!["777.77", "999"]
    );
}
