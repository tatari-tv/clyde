#![allow(clippy::unwrap_used)]

//! The generator's own contract: seeded, reproducible, and carrying every shape the design's three
//! fixtures are specified to carry. If a fixture stops exercising the case it exists for, the check
//! that rides on it silently stops proving anything -- so each case is asserted here rather than
//! trusted to a hand-read of the committed JSON.

use super::*;
use crate::aggregate;
use std::collections::BTreeSet;

fn pricing() -> Pricing {
    Pricing::embedded()
}

/// A fixture is diffable only if it is reproducible: same kind, same bytes, every time. The frozen
/// `generated` stamp is the whole point -- without it `build_report`'s `Utc::now()` makes every
/// regeneration differ in exactly one field.
///
/// BITES: drop the `report.generated = ts(GENERATED)` line and this fails on the timestamp.
#[test]
fn the_same_kind_generates_byte_identical_json() {
    for kind in [Kind::Small, Kind::Medium, Kind::MediumPrior, Kind::Pathological] {
        let a = serde_json::to_string_pretty(&build(kind, &pricing()).unwrap()).unwrap();
        let b = serde_json::to_string_pretty(&build(kind, &pricing()).unwrap()).unwrap();
        assert_eq!(a, b, "{kind:?} is not reproducible from its seed");
    }
}

/// Two fixtures must never share a session id: they are evaluated in one run and a collision would
/// make a citation check pass against the wrong window.
#[test]
fn the_fixtures_share_no_session_ids() {
    let small = build(Kind::Small, &pricing()).unwrap();
    let medium = build(Kind::Medium, &pricing()).unwrap();
    let pathological = build(Kind::Pathological, &pricing()).unwrap();
    for (a, b) in [(&small, &medium), (&small, &pathological), (&medium, &pathological)] {
        let shared: Vec<&String> = a.sessions.keys().filter(|k| b.sessions.contains_key(*k)).collect();
        assert!(shared.is_empty(), "two fixtures share session ids: {shared:?}");
    }
}

/// SMALL is the baseline shape: one repo, no subagents, every session observed by `git-origin`.
#[test]
fn small_is_a_single_repo_with_no_subagents() {
    let report = build(Kind::Small, &pricing()).unwrap();
    let repos: BTreeMap<&str, ()> = report
        .sessions
        .values()
        .filter_map(|s| s.repo.as_deref())
        .map(|r| (r, ()))
        .collect();
    assert_eq!(repos.len(), 1, "small must be a single repo, got {repos:?}");
    assert!(
        report
            .sessions
            .values()
            .all(|s| s.efficiency.subagents.is_empty() && s.repo_source.as_deref() == Some("git-origin")),
        "small must have no subagents and only git-origin attribution"
    );
    assert!(
        report.sessions.values().any(|s| s.title.is_none()),
        "small must still carry an untitled session for the short-id citation"
    );
}

/// MEDIUM must carry a POSITIVE `(main-session)` residual. Design Phase 5 is explicit: if subagents
/// consume the whole aggregate the residual map empties, no `(main-session)` row emits, and the
/// partition criterion never exercises the row this design added.
///
/// BITES: give the parent scope no tokens of its own and this fails.
#[test]
fn medium_carries_a_positive_main_session_residual() {
    let report = build(Kind::Medium, &pricing()).unwrap();
    let delegating: Vec<_> = report
        .sessions
        .values()
        .filter(|s| !s.efficiency.subagents.is_empty())
        .collect();
    assert!(!delegating.is_empty(), "medium must delegate to subagents");
    for entry in delegating {
        let residual = entry
            .agent_type_costs
            .get(crate::report::MAIN_SESSION_BUCKET)
            .expect("a delegating session must emit a (main-session) row");
        assert!(
            residual.tokens > 0 && residual.cost_usd > 0.0,
            "the (main-session) residual must be positive, got {residual:?}"
        );
    }
}

/// MEDIUM must span several orgs, exercise all four `repo-source` values, and carry the full outcome
/// mix -- it is the fixture the coverage dimension is scored on.
#[test]
fn medium_spans_orgs_sources_and_the_full_outcome_mix() {
    let report = build(Kind::Medium, &pricing()).unwrap();
    let aggregates = aggregate::compute(&report, crate::aggregate::DEFAULT_OUTLIERS, &pricing());
    assert!(
        aggregates.by_org.len() >= 3,
        "medium must span at least three orgs, got {:?}",
        aggregates.by_org.iter().map(|o| &o.org).collect::<Vec<_>>()
    );
    let sources: BTreeSet<&str> = report
        .sessions
        .values()
        .filter_map(|s| s.repo_source.as_deref())
        .collect();
    for want in ["git-origin", "known-path", "files-touched", "path-guess"] {
        assert!(sources.contains(want), "medium must exercise {want}, got {sources:?}");
    }
    let totals = report.totals.outcomes.as_ref().expect("medium has an outcome rollup");
    assert!(totals.commits > 0 && totals.prs_opened > 0 && totals.files_edited > 0);
    assert!(totals.lines_written > 0 && totals.lines_replaced > 0);
    assert!(
        report.sessions.values().any(|s| s.summary.is_none()) && report.sessions.values().any(|s| s.summary.is_some()),
        "medium must have partial enrichment so the title-fallback path is exercised"
    );
    assert!(
        report.sessions.values().any(|s| s.title.is_none()),
        "medium must carry untitled sessions"
    );
    assert!(
        report
            .sessions
            .values()
            .any(|s| s.outcomes.as_ref().is_some_and(|o| !o.prs.is_empty())),
        "medium must carry PRs so the prose can reference one"
    );
}

/// MEDIUM's prior period must be the SAME length as medium's, or `prior.comparable` is `false` and
/// the Month over Month section carries a caveat the fixture never meant to exercise.
#[test]
fn the_medium_prior_window_is_the_same_length() {
    let medium = build(Kind::Medium, &pricing()).unwrap();
    let prior = build(Kind::MediumPrior, &pricing()).unwrap();
    let days = |r: &Report| (r.until.date_naive() - r.since.date_naive()).num_days() + 1;
    assert_eq!(days(&medium), days(&prior));
    assert!(prior.until < medium.since, "the prior window must precede the period");
}

/// PATHOLOGICAL is the absent-section fixture: zero outcomes, an unpriced nonzero-token model, a
/// multi-day gap, carried-in sessions, and an attribution that is entirely `path-guess`.
///
/// BITES: give any pathological session an outcome and the zero-outcome assertion fails, which is
/// what keeps `forbid-sections: Quantified Output` a real check rather than a coincidence.
#[test]
fn pathological_carries_every_absent_section_case() {
    let report = build(Kind::Pathological, &pricing()).unwrap();

    assert!(
        report.sessions.values().all(|s| s.outcomes.is_none()),
        "pathological must observe zero outcomes"
    );
    assert_eq!(
        report.totals.untracked_models,
        vec![MODEL_UNPRICED.to_string()],
        "pathological must carry exactly one unpriced model"
    );
    let unpriced = report
        .totals
        .models
        .get(MODEL_UNPRICED)
        .expect("the unpriced model must still appear in totals.models");
    assert!(
        unpriced.total > 0 && unpriced.spend_usd.is_none(),
        "the unpriced model must carry NONZERO tokens (a zero-token model is dropped by Phase 6's gate)"
    );
    assert!(
        report
            .sessions
            .values()
            .all(|s| s.repo_source.as_deref() == Some("path-guess")),
        "pathological attribution must be entirely path-guess"
    );

    let aggregates = aggregate::compute(&report, crate::aggregate::DEFAULT_OUTLIERS, &pricing());
    assert_eq!(aggregates.carried_in.sessions, 3, "pathological must carry sessions in");
    let gap = aggregates.by_day.windows(4).any(|w| w.iter().all(|d| !d.active));
    assert!(gap, "pathological must contain a multi-day gap of inactive rows");
    assert!(
        report.sessions.values().all(|s| s.summary.is_none()),
        "pathological must have zero enrich coverage"
    );
}

/// The synthesized Analytics export must reconcile against the medium report without a window
/// error, and must report BILLED ABOVE MODELED -- the expected relationship, since an export covers
/// account activity clyde never sees.
#[test]
fn the_analytics_export_matches_the_medium_window() {
    let report = build(Kind::Medium, &pricing()).unwrap();
    let export = analytics_export(&report).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("analytics.json");
    std::fs::write(&path, export).unwrap();

    let recon = crate::reconcile::fold(&path, Some(OPERATOR_EMAIL), &report)
        .expect("the synthesized export must match the window");
    let strip = |s: &str| s.replace(['$', ',', '+'], "");
    let billed: f64 = strip(&recon.billed).parse().unwrap();
    let modeled: f64 = strip(&recon.modeled).parse().unwrap();
    assert!(billed > modeled, "billed ({billed}) must exceed modeled ({modeled})");
    // The other seat's $9,702.65 is in the file and in none of these figures. If the operator
    // filter regressed, `billed` would clear ten thousand dollars against a modeled total in the
    // low hundreds -- the org-wide defect, reproduced in miniature.
    assert!(
        billed < 10_000.0,
        "billed ({billed}) must exclude the second actor's rows"
    );
}

/// The synthesized export is scoped to [`OPERATOR_EMAIL`], and a render resolves its operator from
/// the fixture's persona -- so the medium fixture's `eval.yml` persona email MUST be that same
/// address. Drift makes the fixture's reconciliation fail closed ("no row for the operator") on
/// every eval run, which is a confusing way to learn about a one-line typo.
#[test]
fn the_medium_fixture_persona_is_the_export_operator() {
    let spec = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/report/medium/eval.yml"),
    )
    .expect("the medium fixture's eval.yml must exist");
    assert!(
        spec.contains(OPERATOR_EMAIL),
        "fixtures/report/medium/eval.yml must carry persona email {OPERATOR_EMAIL}"
    );
}

/// Nothing in the generator's vocabulary may look like real Tatari data. This is the public-repo
/// rule in test form: a future edit that pastes a real org or repo slug in here fails the build.
#[test]
fn the_vocabulary_names_nothing_real() {
    let mut corpus = String::new();
    for kind in [Kind::Small, Kind::Medium, Kind::MediumPrior, Kind::Pathological] {
        corpus.push_str(&serde_json::to_string(&build(kind, &pricing()).unwrap()).unwrap());
    }
    for forbidden in ["tatari", "saidler", "scottidler", "clyde", "marquee", "philo"] {
        assert!(
            !corpus.to_ascii_lowercase().contains(forbidden),
            "a synthesized fixture names {forbidden:?}; fixtures are invented, never derived"
        );
    }
}

/// The frozen timestamp const is a literal in this file, so a typo in it must fail here rather than
/// panicking inside a fixture regeneration.
#[test]
fn the_generated_const_parses() {
    assert_eq!(ts(GENERATED).to_rfc3339(), "2026-06-01T12:00:00+00:00");
}
