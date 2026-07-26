#![allow(clippy::unwrap_used)]

//! Each check gets a positive case and the negative it exists to catch. Where a check is one half of
//! a design success criterion, the test names the criterion.

use super::*;
use crate::quotable::QuotableFacts;

const CONTEXT: &str = r#"{
  "period": {"since": "2026-04-01", "until": "2026-04-30", "days": 30, "active-days": 21},
  "totals": {"sessions": 44, "spend": "$1,234.56"},
  "sessions": [
    {"short-id": "a14bc3d2", "title": null, "repo": "northwind-media/beacon", "spend-display": "$18.00"},
    {"short-id": "b27de4f1", "title": "Wire the ingest retry backoff", "repo": "northwind-media/tideline",
     "spend-display": "$9.00"}
  ],
  "aggregates": {
    "by-repo": [
      {"repo": "northwind-media/beacon", "org": "northwind-media", "spend": "$700.00"},
      {"repo": "northwind-media/tideline", "org": "northwind-media", "spend": "$300.00"},
      {"repo": "jrivera/sextant", "org": "jrivera", "spend": "$200.00"}
    ],
    "by-day": [{"date": "2026-04-01", "sessions": 3, "spend": "$44.00", "active": true}],
    "charts": {"by-day-spend": {"viewbox": "0 0 1000 300", "points": "0,290 500,10 1000,150",
                                "y-labels": ["$44.00"], "x-labels": ["2026-04-01"]}}
  },
  "efficiency": {"agent-type-costs": [{"name": "(main-session)", "tokens-human": "9.5M", "spend": "$900.00"}]}
}"#;

fn ground() -> Ground {
    Ground::from_context_json(CONTEXT).unwrap()
}

fn context() -> RenderContext {
    RenderContext {
        json: CONTEXT.to_string(),
        facts: QuotableFacts::from_context_json(CONTEXT).unwrap(),
    }
}

fn spec() -> Spec {
    Spec::default()
}

fn checks(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.check.as_str()).collect()
}

/// The ground truth is taken from the serialized block, so "exists in the context" is literally
/// true rather than approximately true.
#[test]
fn ground_truth_comes_from_the_context_block() {
    let g = ground();
    assert!(g.repos.contains("northwind-media/beacon"));
    assert!(g.orgs.contains("jrivera"));
    assert!(g.repo_names.contains("sextant"));
    assert!(g.dates.contains("2026-04-30"));
    assert_eq!(g.untitled_short_ids, BTreeSet::from(["a14bc3d2".to_string()]));
    assert_eq!(
        g.top_repos,
        vec!["northwind-media/beacon", "northwind-media/tideline", "jrivera/sextant"]
    );
    assert_eq!(g.top_agent_type.as_deref(), Some("(main-session)"));
    assert!(g.has_charts);
}

/// A clean artifact produces no findings. Without this the negatives below prove only that the
/// checks fire, not that they discriminate.
#[test]
fn a_clean_markdown_artifact_passes_every_check() {
    let artifact = "# Claude Enterprise Usage Report\n\n\
        **Total Spend:** $1,234.56\n\n\
        ## Cost Summary\n\
        Across 2026-04-01 to 2026-04-30 the window ran 44 sessions in northwind-media/beacon, \
        northwind-media/tideline and jrivera/sextant. The untitled session a14bc3d2 spent $18.00; \
        \"Wire the ingest retry backoff\" spent $9.00 in 30 days, 21 of them active.\n";
    assert_eq!(
        mechanical_checks(artifact),
        Vec::<&str>::new(),
        "a clean artifact must produce no findings"
    );
}

fn mechanical_checks(artifact: &str) -> Vec<String> {
    check(Kind::Markdown, artifact, &context(), &ground(), &spec())
        .into_iter()
        .map(|f| f.check)
        .collect()
}

/// Design success criterion: "deliberately corrupting a golden's narrative (swap a repo name) fails
/// the `otto ci` citation check". The org stays real and only the repo name changes, which is the
/// hardest version of the swap to notice by eye.
///
/// BITES: drop the `ground.orgs.contains(left)` anchor and the swap sails through.
#[test]
fn swapping_a_repo_name_fails_the_citation_check() {
    let corrupted = "## Cost Summary\nWork landed in northwind-media/lighthouse this period.\n";
    let findings = check(Kind::Markdown, corrupted, &context(), &ground(), &spec());
    assert_eq!(checks(&findings), vec!["cited-repos"]);
    assert!(findings[0].detail.contains("northwind-media/lighthouse"));
}

/// The other half of the swap: the repo name stays real and the ORG changes.
#[test]
fn swapping_an_org_fails_the_citation_check() {
    let corrupted = "## Cost Summary\nWork landed in eastwind-media/beacon this period.\n";
    let findings = check(Kind::Markdown, corrupted, &context(), &ground(), &spec());
    assert_eq!(checks(&findings), vec!["cited-repos"]);
}

/// A repo slug inside a URL is checked too, so a corrupted PR link cannot slip past on its path
/// depth alone.
#[test]
fn a_corrupted_slug_inside_a_url_is_caught() {
    let corrupted = "See https://github.com/northwind-media/lighthouse/pull/118 for the change.\n";
    let findings = check(Kind::Markdown, corrupted, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"cited-repos"), "{findings:?}");
    // ...and the correct form of the same link passes, so the check is not simply rejecting URLs.
    let honest = "See https://github.com/northwind-media/beacon/pull/118 for the change.\n";
    assert!(!checks(&check(Kind::Markdown, honest, &context(), &ground(), &spec())).contains(&"cited-repos"));
}

/// Ordinary prose containing a slash is NOT a repo citation. The check is anchored on the context's
/// own vocabulary precisely so `read/write` and `cache-read/cache-write` never trip it -- a false
/// positive here is a hard render failure.
#[test]
fn ordinary_slashed_prose_is_not_a_repo_citation() {
    let prose = "The cache-read/cache-write split and the input/output ratio are unchanged; \
                 and/or either way, n/a.\n";
    let findings = check(Kind::Markdown, prose, &context(), &ground(), &spec());
    assert!(!checks(&findings).contains(&"cited-repos"), "{findings:?}");
}

#[test]
fn a_date_outside_the_context_is_caught() {
    let artifact = "Work clustered on 2026-04-14, with a spike on 2026-09-09.\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    let dates: Vec<&Finding> = findings.iter().filter(|f| f.check == "cited-dates").collect();
    assert_eq!(dates.len(), 2, "both dates are absent from the context: {findings:?}");
    // The one date the context DOES carry passes.
    let honest = "Work clustered on 2026-04-01.\n";
    assert!(!checks(&check(Kind::Markdown, honest, &context(), &ground(), &spec())).contains(&"cited-dates"));
}

/// Quoting is a claim of verbatim provenance, so a quoted title the context does not carry is a
/// fabricated citation.
#[test]
fn a_fabricated_quoted_title_is_caught() {
    let artifact = "The costliest session was \"Rewrite the billing pipeline end to end\".\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"cited-titles"), "{findings:?}");
}

/// The frontmatter `title:` is a composite the prompt tells the model to ASSEMBLE, so it is quoted
/// without citing anything. Excluding the block keeps a required structural element from reading as
/// a fabricated citation, and the facts inside it are still covered by `cited-dates`.
///
/// BITES: drop `strip_frontmatter` and every markdown golden fails `cited-titles`.
#[test]
fn the_frontmatter_title_is_not_a_citation() {
    let artifact = "---\ntitle: \"Claude Usage Report - Jordan Rivera - 2026-04-01 - 2026-04-30\"\n\
        date: 2026-04-30\n---\n\n## Cost Summary\nThe window ran 44 sessions.\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    assert!(!checks(&findings).contains(&"cited-titles"), "{findings:?}");

    // ...and a fabricated quote AFTER the block is still caught.
    let artifact = format!("{artifact}\nThe session \"Rewrite the billing pipeline end to end\" led.\n");
    let findings = check(Kind::Markdown, &artifact, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"cited-titles"), "{findings:?}");
}

/// A short quoted fragment is not a citation and is deliberately skipped: demanding every quoted
/// word appear in the context would reject ordinary emphasis.
#[test]
fn a_short_quoted_fragment_is_not_treated_as_a_citation() {
    let artifact = "The row is marked \"guessed\" rather than observed.\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    assert!(!checks(&findings).contains(&"cited-titles"), "{findings:?}");
}

/// Hard prohibition 2's phrase list, in mechanical form.
#[test]
fn hard_prohibition_two_phrases_are_caught() {
    for banned in [
        "This is roughly a senior engineer's monthly cost.",
        "It would have required 200 hours of work.",
        "The ROI is clear.",
        "That is half an FTE.",
        "It pays for itself.",
    ] {
        let findings = check(Kind::Markdown, banned, &context(), &ground(), &spec());
        assert!(
            checks(&findings).contains(&"speculative-quantification"),
            "{banned:?} must be rejected, got {findings:?}"
        );
    }
}

/// Word boundaries, not substrings: a bare `contains` scan makes `fte` match "after".
///
/// BITES: drop the `\b` anchors from `speculative_pattern` and this fails.
#[test]
fn a_banned_phrase_inside_a_longer_word_is_not_a_match() {
    let prose = "After the rollout, the aftercare notes were filed.\n";
    let findings = check(Kind::Markdown, prose, &context(), &ground(), &spec());
    assert!(
        !checks(&findings).contains(&"speculative-quantification"),
        "{findings:?}"
    );
}

#[test]
fn an_em_dash_is_caught_anywhere_in_the_artifact() {
    let artifact = "The window ran 44 sessions \u{2014} all of them in one repo.\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"em-dash"), "{findings:?}");
}

/// The render-invents-nothing guard, run through the eval rather than through `render`.
#[test]
fn a_fabricated_figure_is_caught() {
    let artifact = "The team shipped 9,912 commits this period.\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"foreign-figures"), "{findings:?}");
}

#[test]
fn required_and_forbidden_sections_are_enforced() {
    let spec = Spec {
        require_sections: vec!["Cost Summary".into(), "Month over Month".into()],
        forbid_sections: vec!["Quantified Output".into()],
        ..Spec::default()
    };
    let artifact = "## Cost Summary\nbody\n\n## Quantified Output\nbody\n";
    let findings = check(Kind::Markdown, artifact, &context(), &ground(), &spec);
    let names = checks(&findings);
    assert!(names.contains(&"required-sections"), "{findings:?}");
    assert!(names.contains(&"forbidden-sections"), "{findings:?}");
}

/// Phase 10's criterion 3, in the form Phase 13 owes it: the fixture DECLARES the two citation
/// shapes a narrowed whitelist breaks first, and a golden that never exercises them fails rather
/// than passing vacuously.
#[test]
fn required_citations_must_actually_appear() {
    let spec = Spec {
        require_citations: vec![Citation::UntitledShortId, Citation::PrReference],
        ..Spec::default()
    };
    let missing = "## Usage Profile\nThe period was steady.\n";
    let findings = check(Kind::Markdown, missing, &context(), &ground(), &spec);
    assert_eq!(
        findings.iter().filter(|f| f.check == "required-citations").count(),
        2,
        "{findings:?}"
    );

    let present = "## Usage Profile\nUntitled session a14bc3d2 landed in #118.\n";
    let findings = check(Kind::Markdown, present, &context(), &ground(), &spec);
    assert!(!checks(&findings).contains(&"required-citations"), "{findings:?}");
}

/// The html path scans VISIBLE TEXT, so authored CSS numbers are not data figures -- and the
/// geometry allowlist runs over the markup that `visible_text` throws away.
#[test]
fn the_html_path_scans_visible_text_and_validates_geometry() {
    let clean = "<!doctype html><html><head><style>.bar{width:63.5%}</style></head><body>\
        <h1>44 sessions, $1,234.56</h1>\
        <svg viewBox=\"0 0 1000 300\" class=\"chart\"><polyline points=\"0,290 500,10 1000,150\" \
        class=\"line\"/></svg></body></html>";
    let findings = check(Kind::Html, clean, &context(), &ground(), &spec());
    assert_eq!(findings, Vec::new(), "a clean html artifact must pass: {findings:?}");

    let fabricated = clean.replace("0,290 500,10 1000,150", "0,111 500,22 1000,33");
    let findings = check(Kind::Html, &fabricated, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"chart-geometry"), "{findings:?}");
}

/// The positive half of the geometry check: when the binary computed a chart, an artifact that
/// silently dropped it is a finding, not a pass.
#[test]
fn html_without_a_polyline_is_a_finding_when_the_context_has_charts() {
    let chartless = "<!doctype html><html><body><h1>44 sessions, $1,234.56</h1></body></html>";
    let findings = check(Kind::Html, chartless, &context(), &ground(), &spec());
    assert!(checks(&findings).contains(&"chart-geometry"), "{findings:?}");
}
