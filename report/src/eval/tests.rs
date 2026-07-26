#![allow(clippy::unwrap_used)]

//! The `otto ci` half of the eval: the mechanical layer, run offline and free against the COMMITTED
//! goldens (design Phase 13's first success criterion). Nothing here calls a model or touches the
//! network -- a golden either passes forever or the change that broke it is in this repo.

use super::*;
use crate::eval::fixture::{Citation, Dimension};
use crate::eval::mechanical::{self, Ground, Kind};
use std::collections::BTreeSet;

/// The workspace root, resolved from the crate rather than from the process CWD (which is the crate
/// directory under `cargo test` and the workspace root under `otto`).
fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The three committed fixtures, loaded.
fn fixtures() -> Vec<Fixture> {
    fixture::committed_dirs(&workspace())
        .iter()
        .map(|dir| Fixture::load(dir).expect("a committed fixture must load"))
        .collect()
}

/// Load a fixture and rebuild the exact context its goldens were rendered from.
fn loaded(fixture: &Fixture) -> (Report, RenderContext, Ground) {
    let pricing = Pricing::embedded();
    let report = render::load_report(&fixture.report, "fixture report").unwrap();
    let context = build_context(fixture, &report, &pricing).unwrap();
    let ground = Ground::from_context_json(&context.json).unwrap();
    (report, context, ground)
}

/// **Design Phase 13, success criterion 1:** the mechanical layer runs on all three goldens,
/// offline and green.
#[test]
fn every_committed_golden_passes_the_mechanical_layer() {
    for fixture in fixtures() {
        let (_, context, ground) = loaded(&fixture);
        let markdown = fixture
            .golden_markdown
            .as_ref()
            .unwrap_or_else(|| panic!("fixture {} must commit a markdown golden", fixture.name));
        let findings = mechanical::check(Kind::Markdown, markdown, &context, &ground, &fixture.spec);
        assert!(
            findings.is_empty(),
            "markdown golden for {} failed the mechanical layer: {findings:#?}",
            fixture.name
        );

        let html = fixture
            .golden_html
            .as_ref()
            .unwrap_or_else(|| panic!("fixture {} must commit an html golden", fixture.name));
        let findings = mechanical::check(Kind::Html, html, &context, &ground, &fixture.spec);
        assert!(
            findings.is_empty(),
            "html golden for {} failed the mechanical layer: {findings:#?}",
            fixture.name
        );
    }
}

/// **Design Phase 13, success criterion 3:** deliberately corrupting a golden's narrative (swap a
/// repo name) fails the `otto ci` citation check. Run against every golden that names a repo, so
/// the criterion holds for the whole corpus rather than for one hand-picked artifact.
///
/// BITES: this is the same check `every_committed_golden_passes_the_mechanical_layer` relies on, so
/// a guard that stopped discriminating would fail HERE while the green test above kept passing.
#[test]
fn corrupting_a_golden_repo_name_fails_the_citation_check() {
    for fixture in fixtures() {
        let (_, context, ground) = loaded(&fixture);
        let markdown = fixture.golden_markdown.as_ref().unwrap();
        let (org, name) = ground
            .top_repos
            .first()
            .and_then(|slug| slug.split_once('/'))
            .expect("every fixture has a top by-repo row");
        assert!(
            markdown.contains(&format!("{org}/{name}")),
            "the {} golden must name its top repo before it can be corrupted",
            fixture.name
        );
        let corrupted = markdown.replace(&format!("{org}/{name}"), &format!("{org}/lighthouse"));
        let findings = mechanical::check(Kind::Markdown, &corrupted, &context, &ground, &fixture.spec);
        assert!(
            findings.iter().any(|f| f.check == "cited-repos"),
            "the corrupted {} golden must fail cited-repos, got {findings:#?}",
            fixture.name
        );
    }
}

/// **Design Phase 10, success criterion 3, re-run against the real goldens.** Phase 10 met this
/// against three artifacts it built for itself, because Phase 13's goldens did not exist yet, and
/// wrote the re-run requirement into its own test's doc comment. This is that re-run: every figure
/// in all three committed goldens passes the quotable-facts guard, and the corpus provably includes
/// an untitled session cited by `short-id` and a prose PR reference.
#[test]
fn phase_ten_criterion_three_holds_against_the_committed_goldens() {
    let mut saw_untitled = false;
    let mut saw_pr = false;
    for fixture in fixtures() {
        let (_, context, ground) = loaded(&fixture);
        for (kind, artifact) in [
            (Kind::Markdown, fixture.golden_markdown.as_ref().unwrap()),
            (Kind::Html, fixture.golden_html.as_ref().unwrap()),
        ] {
            let prose = match kind {
                Kind::Markdown => artifact.clone(),
                Kind::Html => crate::render::visible_text(artifact),
            };
            let foreign = context.facts.foreign_figures(&prose);
            assert!(
                foreign.is_empty(),
                "{} {} golden states figures no quotable fact licenses: {foreign:?}",
                fixture.name,
                kind.as_str()
            );
        }
        let markdown = fixture.golden_markdown.as_ref().unwrap();
        saw_untitled |= ground.untitled_short_ids.iter().any(|sid| markdown.contains(sid));
        saw_pr |= markdown.contains("/pull/") || markdown.contains('#');
    }
    assert!(
        saw_untitled,
        "no golden cites an untitled session by short-id; the whitelist's first false positive is unproven"
    );
    assert!(
        saw_pr,
        "no golden carries a prose PR reference; the whitelist's second false positive is unproven"
    );
}

/// Every fixture must actually contain the two cases the criterion above rests on, in the DATA and
/// not merely in the golden: a fixture with no untitled session and no PR could never exercise them
/// however the model renders it.
#[test]
fn the_fixtures_contain_the_untitled_and_pr_cases() {
    let mut with_untitled = 0usize;
    let mut with_prs = 0usize;
    for fixture in fixtures() {
        let (report, _, ground) = loaded(&fixture);
        if !ground.untitled_short_ids.is_empty() {
            with_untitled += 1;
        }
        if report
            .sessions
            .values()
            .any(|s| s.outcomes.as_ref().is_some_and(|o| !o.prs.is_empty()))
        {
            with_prs += 1;
        }
    }
    assert_eq!(with_untitled, 3, "every fixture must carry an untitled session");
    assert!(with_prs >= 1, "at least one fixture must carry PRs");
}

/// At least one committed fixture must REQUIRE both citation shapes, or the requirement is a field
/// nobody sets and the criterion is met by accident rather than by contract.
#[test]
fn a_committed_fixture_requires_both_citation_shapes() {
    let required: BTreeSet<&str> = fixtures()
        .iter()
        .flat_map(|f| f.spec.require_citations.clone())
        .map(Citation::as_str)
        .collect();
    assert!(required.contains(Citation::UntitledShortId.as_str()));
    assert!(required.contains(Citation::PrReference.as_str()));
}

/// Every committed fixture must commit a floor for every judged dimension. A dimension with no
/// floor cannot regress, so an unset one is a silently unmeasured dimension.
#[test]
fn every_committed_fixture_commits_a_floor_for_every_dimension() {
    for fixture in fixtures() {
        for dimension in Dimension::ALL {
            assert!(
                fixture.spec.floors.contains_key(dimension),
                "fixture {} sets no floor for {}",
                fixture.name,
                dimension.as_str()
            );
        }
    }
}

/// The eval pins EMBEDDED pricing so a fixture is reproducible offline, which couples the committed
/// goldens to the rates in `pricing/data/pricing.json`. That coupling is real, so it gets a named
/// failure rather than a mysterious one: if a pricing refresh moves any rate a fixture's models are
/// priced at, every dollar figure in the context moves and the goldens must be re-rendered.
///
/// Remedy, when this fails:
/// ```text
/// cargo run -p report --bin fixtures -- fixtures/report
/// otto eval          # re-render and re-judge, then commit the new goldens
/// ```
#[test]
fn fixture_models_still_carry_the_rates_the_goldens_were_rendered_against() {
    let pricing = Pricing::embedded();
    // (model, input, output, cache-5m-write, cache-1h-write, cache-read) per Mtok.
    let pinned = [
        ("claude-opus-4-7", 5.0, 25.0, 6.25, 10.0, 0.5),
        ("claude-sonnet-4-6", 3.0, 15.0, 3.75, 6.0, 0.3),
        ("claude-haiku-4-5", 1.0, 5.0, 1.25, 2.0, 0.1),
    ];
    for (model, input, output, write_5m, write_1h, read) in pinned {
        let rates = pricing
            .lookup(model)
            .unwrap_or_else(|| panic!("{model} left the pricing feed; re-render the goldens"));
        assert_eq!(
            (
                rates.input_per_mtok,
                rates.output_per_mtok,
                rates.cache_5m_write_per_mtok,
                rates.cache_1h_write_per_mtok,
                rates.cache_read_per_mtok
            ),
            (input, output, write_5m, write_1h, read),
            "{model}'s rates moved; the committed goldens were rendered against the old ones -- \
             regenerate the fixtures and re-render (see this test's doc comment)"
        );
    }
    assert!(
        pricing.lookup(synth::MODEL_UNPRICED).is_none(),
        "the pathological fixture's untracked-model case needs a model with no price"
    );
}

/// A fixture's committed `report.json` must be exactly what the seeded generator produces today.
/// Without this the generator and the fixtures drift, and a "seeded, reproducible" fixture becomes
/// a file nobody can regenerate.
///
/// BITES: change any generator constant and this fails, naming the fixture and the remedy.
#[test]
fn the_committed_fixtures_match_the_generator() {
    let pricing = Pricing::embedded();
    for kind in [synth::Kind::Small, synth::Kind::Medium, synth::Kind::Pathological] {
        let Some(name) = kind.dir_name() else { continue };
        let path = workspace().join(fixture::FIXTURE_ROOT).join(name).join("report.json");
        let committed = std::fs::read_to_string(&path).unwrap();
        let regenerated = serde_json::to_string_pretty(&synth::build(kind, &pricing).unwrap()).unwrap() + "\n";
        assert_eq!(
            committed, regenerated,
            "fixtures/report/{name}/report.json is not what the generator produces; re-run \
             `cargo run -p report --bin fixtures -- fixtures/report` and re-render the goldens"
        );
    }
}

/// `--out` is written even when the eval is going to fail, so the evidence for the failure is on
/// disk before the failure is raised.
#[test]
fn the_scored_report_serializes() {
    let report = EvalReport {
        generated: Utc::now(),
        judge_model: "test-model".into(),
        pricing_basis: "embedded pricing feed, data-version test".into(),
        fixtures: vec![FixtureOutcome {
            name: "small".into(),
            summary: "a fixture".into(),
            sessions: 9,
            spend: "$1.00".into(),
            markdown_ok: true,
            markdown_error: None,
            markdown_findings: Vec::new(),
            html_ok: false,
            html_error: Some("preserveaspectratio is not permitted".into()),
            html_findings: Vec::new(),
            verdict: None,
            regressions: vec![Regression {
                dimension: "coverage".into(),
                score: 1,
                floor: 2,
                reason: "the top by-repo row is never named".into(),
            }],
            passed: false,
        }],
        guards: Guards {
            markdown_renders: 3,
            markdown_rejections: 0,
            markdown_rejection_rate: "0.0%".into(),
            html_renders: 3,
            html_rejections: 1,
            html_rejection_rate: "33.3%".into(),
            reasons: vec!["small (html): preserveaspectratio".into()],
        },
        passed: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("eval-report.json");
    write_report(&report, &out).unwrap();
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(body.contains("\"html-rejection-rate\": \"33.3%\""), "{body}");
    assert!(body.contains("\"passed\": false"), "{body}");
}

#[test]
fn the_rejection_rate_reads_as_a_percent_and_never_divides_by_zero() {
    assert_eq!(rate(0, 0), "n/a");
    assert_eq!(rate(1, 3), "33.3%");
    assert_eq!(rate(0, 3), "0.0%");
}

/// An empty fixture set is a loud error rather than a vacuous pass.
#[test]
fn an_empty_fixture_set_fails() {
    let cfg = EvalConfig {
        fixtures: Vec::new(),
        judge_model: "test-model".into(),
        out: PathBuf::from("/dev/null"),
        write_goldens: false,
        llm: Llm::Api,
        markdown_model: "m".into(),
        html_model: "h".into(),
        markdown_max_output_tokens: 1_024,
        html_max_output_tokens: 1_024,
    };
    let err = run(&cfg, &Pricing::embedded()).unwrap_err().to_string();
    assert!(err.contains("--fixture"), "{err}");
}

/// **Design Phase 13, success criterion 4:** a judged fresh render that misses the top `by-repo`
/// row drops below its coverage floor and exits non-zero.
///
/// Ignored by default because it is PAID: it judges a deliberately mutilated golden against the
/// live api. Run it with
/// `ANTHROPIC_API_KEY=... cargo test -p report -- --ignored --nocapture coverage_floor`.
///
/// The mutilation is the criterion's own: every line naming the top `by-repo` row is deleted from a
/// golden that scored 3 on coverage, and nothing else changes. The offline half of this criterion
/// (a below-floor score IS a regression, and a regression IS a non-zero exit) is pinned by
/// `judge::tests::a_coverage_score_below_the_floor_is_a_regression` and `run`'s own `bail!`; this is
/// the half only a real judge can answer.
#[test]
#[ignore = "paid: judges a mutilated golden against the live api; needs ANTHROPIC_API_KEY"]
fn a_render_missing_the_top_repo_scores_below_its_coverage_floor() {
    let fixture = Fixture::load(&workspace().join(fixture::FIXTURE_ROOT).join("medium")).unwrap();
    let (_, context, ground) = loaded(&fixture);
    let top = ground.top_repos.first().expect("medium has a top by-repo row").clone();

    let golden = fixture.golden_markdown.as_ref().unwrap();
    let mutilated: String = golden
        .lines()
        .filter(|line| !line.contains(&top))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !mutilated.contains(&top) && golden.contains(&top),
        "the mutilation must actually remove the top repo {top}"
    );

    let brief = judge::brief(&context.json).unwrap();
    let verdict = judge::score(
        &ApiTransport::from_env().expect("ANTHROPIC_API_KEY must be set for this ignored test"),
        common::config::DEFAULT_MARKDOWN_MODEL,
        common::config::DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS,
        &mutilated,
        &brief,
    )
    .unwrap();
    let floor = fixture.spec.floor(Dimension::Coverage);
    println!(
        "coverage floor probe: removed {top}, judge scored coverage={} against floor {floor} -- {}",
        verdict.coverage.score, verdict.coverage.reason
    );
    assert!(
        verdict.coverage.score < floor,
        "a render missing the top by-repo row must fall below the coverage floor: scored {} against {floor}",
        verdict.coverage.score
    );
    assert!(
        !verdict.regressions(&fixture.spec).is_empty(),
        "and that score must be reported as a regression, which is what exits non-zero"
    );
}
