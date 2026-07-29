#![allow(clippy::unwrap_used)]

use super::*;

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

const MINIMAL_REPORT: &str = r#"{"schema-version":2}"#;

/// The local-fixture case: a directory holding only a real month's `report.json`, no spec and no
/// goldens. It must load, so `--fixture <local-dir>` works on an uncommitted directory (design
/// Phase 13: the real-data eval stays local and `fixtures/report/local/` is gitignored).
#[test]
fn a_bare_directory_loads_with_default_spec() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), REPORT_FILE, MINIMAL_REPORT);

    let fixture = Fixture::load(dir.path()).unwrap();
    assert!(fixture.spec.require_sections.is_empty());
    assert!(fixture.spec.floors.is_empty());
    assert!(fixture.spec.persona.is_none());
    assert!(fixture.prior.is_none() && fixture.analytics.is_none());
}

/// A directory with no `report.json` is a LOUD error. Skipping it would let an eval announce "all
/// fixtures passed" having evaluated none of them.
#[test]
fn a_directory_without_a_report_fails_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let err = Fixture::load(dir.path()).unwrap_err().to_string();
    assert!(err.contains(REPORT_FILE), "the error must name the missing file: {err}");
}

#[test]
fn a_missing_directory_names_the_remedy() {
    let err = Fixture::load(Path::new("/nonexistent/fixture/dir"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("--fixture"), "the error must name the remedy: {err}");
}

/// Every optional companion file is picked up when present.
#[test]
fn optional_companions_are_discovered() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), REPORT_FILE, MINIMAL_REPORT);
    write(dir.path(), PRIOR_FILE, MINIMAL_REPORT);
    write(dir.path(), ANALYTICS_FILE, "[]");
    write(dir.path(), GOLDEN_MARKDOWN, "# artifact\n");
    write(
        dir.path(),
        SPEC_FILE,
        "summary: a spec\nrequire-sections: [Cost Summary]\nfloors:\n  coverage: 2\n",
    );

    let fixture = Fixture::load(dir.path()).unwrap();
    assert!(fixture.prior.is_some() && fixture.analytics.is_some());
    assert_eq!(fixture.golden_markdown.as_deref(), Some("# artifact\n"));
    assert_eq!(fixture.spec.require_sections, vec!["Cost Summary".to_string()]);
    assert_eq!(fixture.spec.floor(Dimension::Coverage), 2);
    assert_eq!(fixture.spec.floor(Dimension::Readability), 0, "an unset floor is zero");
}

/// `deny_unknown_fields` in test form: a typo'd key would silently drop a required section or a
/// floor, and a floor nobody enforces is the unmeasured quality this phase exists to remove.
///
/// BITES: remove `deny_unknown_fields` from `Spec` and this parses cleanly.
#[test]
fn a_typo_in_the_spec_is_a_loud_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), REPORT_FILE, MINIMAL_REPORT);
    write(
        dir.path(),
        SPEC_FILE,
        "summary: oops\nrequire-section: [Cost Summary]\n",
    );

    let err = Fixture::load(dir.path()).unwrap_err().to_string();
    assert!(err.contains("eval.yml"), "the error must name the spec file: {err}");
}

/// The dimension vocabulary is shared between `eval.yml`'s floors and the judge's verdict, so a
/// rename in one place that missed the other would silently stop enforcing a floor.
#[test]
fn every_dimension_round_trips_through_its_kebab_spelling() {
    for dimension in Dimension::ALL {
        let parsed: Dimension = serde_yaml::from_str(dimension.as_str()).unwrap();
        assert_eq!(parsed, *dimension);
    }
}

#[test]
fn every_citation_round_trips_through_its_kebab_spelling() {
    for citation in [Citation::UntitledShortId, Citation::PrReference] {
        let parsed: Citation = serde_yaml::from_str(citation.as_str()).unwrap();
        assert_eq!(parsed, citation);
    }
}

#[test]
fn committed_dirs_resolves_the_three_named_fixtures() {
    let dirs = committed_dirs(Path::new("/workspace"));
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/workspace/fixtures/report/small"),
            PathBuf::from("/workspace/fixtures/report/medium"),
            PathBuf::from("/workspace/fixtures/report/pathological"),
        ]
    );
}
