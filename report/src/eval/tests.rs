#![allow(clippy::unwrap_used)]

//! The `otto ci` half of the eval: the mechanical layer, run offline and free against the COMMITTED
//! goldens (design Phase 13's first success criterion). Nothing here calls a model or touches the
//! network -- a golden either passes forever or the change that broke it is in this repo.

use super::*;
use crate::eval::fixture::{Citation, Dimension};
use crate::eval::mechanical::{self, Ground};
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
///
/// Accumulates across EVERY fixture before asserting, rather than panicking on the first failure: a
/// stale golden usually has siblings, and regenerating them one `otto ci` run at a time costs a
/// render per cycle to learn what one run could have said outright.
#[test]
fn every_committed_golden_passes_the_mechanical_layer() {
    let mut failures: Vec<String> = Vec::new();
    for fixture in fixtures() {
        let (_, context, ground) = loaded(&fixture);
        let artifact = fixture
            .golden_markdown
            .as_ref()
            .unwrap_or_else(|| panic!("fixture {} must commit a golden", fixture.name));
        let findings = mechanical::check(artifact, &context, &ground, &fixture.spec);
        if !findings.is_empty() {
            failures.push(format!("{} golden: {findings:#?}", fixture.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden(s) failed the mechanical layer:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The strongest regression net this feature has: a STUBBED render of each fixture must equal its
/// committed `golden.md` BYTE FOR BYTE.
///
/// This is what the inversion bought. The artifact is Rust-authored, so it is reproducible, so a
/// golden is a real fixture rather than a sample of one stochastic render -- and this check runs
/// offline, for free, in `otto ci`. Any change to the document layer that moves a byte shows up
/// here as a diff instead of as a paid render three weeks later.
///
/// Regenerate with `cargo run -p clyde -- report eval --write-goldens` (which needs no transport for
/// the golden itself), then read the diff before committing it.
#[test]
fn a_stubbed_render_reproduces_every_committed_golden_byte_for_byte() {
    let pricing = Pricing::embedded();
    let mut stale: Vec<String> = Vec::new();
    for fixture in fixtures() {
        let report = render::load_report(&fixture.report, "fixture report").unwrap();
        let rendered = render::for_eval(
            &report,
            &pricing,
            fixture.spec.persona.as_ref(),
            eval_opts(&fixture),
            DEFAULT_OUTLIERS,
            render::SlotSource::Stubbed,
        )
        .unwrap();
        let golden = fixture
            .golden_markdown
            .as_ref()
            .unwrap_or_else(|| panic!("fixture {} must commit a golden", fixture.name));
        if &rendered.markdown != golden {
            stale.push(format!(
                "{}: rendered {} bytes, golden {} bytes; first difference at byte {}",
                fixture.name,
                rendered.markdown.len(),
                golden.len(),
                rendered
                    .markdown
                    .bytes()
                    .zip(golden.bytes())
                    .position(|(a, b)| a != b)
                    .map_or_else(
                        || "the end (one is a prefix of the other)".to_string(),
                        |n| n.to_string()
                    ),
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "{} golden(s) no longer match a stubbed render:\n{}\nRegenerate with `report eval --write-goldens`.",
        stale.len(),
        stale.join("\n")
    );
}

/// A stubbed render is DETERMINISTIC: two of them are identical. Without this, the byte-exact golden
/// check above could pass by luck on one run and fail on the next.
#[test]
fn two_stubbed_renders_of_one_fixture_are_identical() {
    let pricing = Pricing::embedded();
    for fixture in fixtures() {
        let report = render::load_report(&fixture.report, "fixture report").unwrap();
        let once = render::for_eval(
            &report,
            &pricing,
            fixture.spec.persona.as_ref(),
            eval_opts(&fixture),
            DEFAULT_OUTLIERS,
            render::SlotSource::Stubbed,
        )
        .unwrap();
        let twice = render::for_eval(
            &report,
            &pricing,
            fixture.spec.persona.as_ref(),
            eval_opts(&fixture),
            DEFAULT_OUTLIERS,
            render::SlotSource::Stubbed,
        )
        .unwrap();
        assert_eq!(once.markdown, twice.markdown, "{} is not deterministic", fixture.name);
        assert!(once.prose.is_empty(), "a stubbed render calls no model");
        assert_eq!(once.attempted, 0, "a stubbed render attempts no slot");
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
        let findings = mechanical::check(&corrupted, &context, &ground, &fixture.spec);
        assert!(
            findings.iter().any(|f| f.check == "cited-repos"),
            "the corrupted {} golden must fail cited-repos, got {findings:#?}",
            fixture.name
        );
    }
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

/// At least one committed fixture must REQUIRE the untitled-short-id shape, or the requirement is a
/// field nobody sets and the rule is met by accident rather than by contract.
///
/// `pr-reference` is deliberately NOT required any more. It existed to prove the quotable-facts
/// whitelist did not false-positive on a prose `#118`; that whitelist is deleted, and the document
/// layer states observed PRs as counts rather than as `#N`, so requiring it would be requiring
/// something the renderer structurally cannot emit. The MATCHER is still exercised directly by
/// `the_pr_reference_matcher_accepts_the_documented_shapes`.
#[test]
fn a_committed_fixture_requires_the_untitled_short_id_shape() {
    let required: BTreeSet<&str> = fixtures()
        .iter()
        .flat_map(|f| f.spec.require_citations.clone())
        .map(Citation::as_str)
        .collect();
    assert!(required.contains(Citation::UntitledShortId.as_str()));
    assert!(
        !required.contains(Citation::PrReference.as_str()),
        "a fixture requiring a prose PR reference can never pass: the document layer writes counts"
    );
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
            slots_attempted: 4,
            slots_degraded: 1,
            verdict: None,
            judge_error: None,
            load_error: None,
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
            markdown_failures: 0,
            slots_attempted: 12,
            slots_degraded: 4,
            slot_degradation_rate: "33.3%".into(),
            reasons: vec!["small: 1 slot(s) shipped empty".into()],
        },
        passed: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("eval-report.json");
    write_report(&report, &out).unwrap();
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(body.contains("\"slot-degradation-rate\": \"33.3%\""), "{body}");
    assert!(body.contains("\"slots-degraded\": 1"), "{body}");
    assert!(body.contains("\"passed\": false"), "{body}");
}

#[test]
fn the_degradation_rate_reads_as_a_percent_and_never_divides_by_zero() {
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
        model: "m".into(),
        judge_max_output_tokens: 1_024,
        slot_max_output_tokens: 512,
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
        common::config::DEFAULT_MODEL,
        common::config::DEFAULT_JUDGE_MAX_OUTPUT_TOKENS,
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

/// The criterion-3 PR proof only means something if the matcher can say NO. The earlier form
/// (`markdown.contains('#')`) said yes to every `##` heading, so `saw_pr` was unconditionally true.
/// Pin the matcher's discrimination directly: heading-shaped prose is not a PR reference, and each
/// shape `Citation::PrReference` documents is.
#[test]
fn the_pr_reference_matcher_rejects_markdown_headings() {
    let pr = mechanical::pr_pattern();

    // What made the old assertion vacuous.
    assert!(
        !pr.is_match("## Spend against output\n\n### By repo\n"),
        "markdown headings are not PR references"
    );
    assert!(!pr.is_match("no references here at all"));
    assert!(
        !pr.is_match("channel #general"),
        "a bare `#` with no digits is not a PR"
    );

    // Each shape the enum's doc comment names.
    assert!(pr.is_match("closed by #62"));
    assert!(pr.is_match("landed in PR 62"));
    assert!(pr.is_match("PRs 61 and 62"));
    assert!(pr.is_match("https://github.com/tatari-tv/clyde/pull/62"));
}

/// A fixture that cannot be loaded is RECORDED as a failed outcome, never propagated: the run must
/// still reach `write_report` so the earlier fixtures' paid renders land on disk.
#[test]
fn an_unloadable_fixture_becomes_a_failed_outcome_named_for_its_directory() {
    let err = eyre::eyre!("eval.yml: missing field `summary`");
    let outcome = FixtureOutcome::unloadable(Path::new("/fixtures/report/pathological"), &err);

    assert_eq!(
        outcome.name, "pathological",
        "named from the directory, not the unread eval.yml"
    );
    assert!(!outcome.passed, "an unevaluated fixture never passes");
    assert_eq!(outcome.sessions, 0);
    let recorded = outcome.load_error.expect("the reason is carried, not swallowed");
    assert!(
        recorded.contains("missing field `summary`"),
        "the underlying error must survive into the report: {recorded}"
    );
    assert!(outcome.judge_error.is_none(), "a load failure is not a judge failure");
}

/// A judge failure fails its fixture but is carried as data, so `run` can keep going and write the
/// report. `passed` must consult it -- otherwise an unscored artifact would pass on the strength of
/// its clean mechanical findings alone.
#[test]
fn a_recorded_judge_error_fails_the_fixture_and_serializes() {
    let outcome = FixtureOutcome {
        name: "small".into(),
        summary: "a fixture".into(),
        sessions: 9,
        spend: "$1.00".into(),
        markdown_ok: true,
        markdown_error: None,
        markdown_findings: Vec::new(),
        slots_attempted: 4,
        slots_degraded: 0,
        verdict: None,
        judge_error: Some("transport: 529 overloaded".into()),
        load_error: None,
        regressions: Vec::new(),
        // The condition `evaluate` computes: everything mechanical is clean, but the artifact went
        // unscored, so the fixture did not pass.
        passed: false,
    };
    let json = serde_json::to_string(&outcome).unwrap();
    assert!(json.contains("judge-error"), "kebab-case, and present: {json}");
    assert!(
        !json.contains("load-error"),
        "an absent reason is omitted, not rendered null: {json}"
    );
}
