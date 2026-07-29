//! Phase 8 (`--prior` and Month over Month): tests split out per the file-size limit.
use super::*;
use crate::render::document::predates_fidelity_fields;

/// Serialize `report` to `<dir>/<name>` and return the path, so a `--prior` fixture is a real
/// schema-gated file on disk rather than an in-memory shortcut.
fn write_report_json(dir: &TempDir, name: &str, report: &Report) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, serde_json::to_string(report).unwrap()).unwrap();
    path
}

/// A report with `repo` set on a session but no `repo_source` anywhere -- a pre-Phase-3 artifact,
/// collected before this design persisted repo-source provenance. Used to exercise
/// [`predates_fidelity_fields`] and the `--prior` caveat it drives.
fn pre_change_report() -> Report {
    let mut report = sample_report();
    let entry = report.sessions.get_mut("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042").unwrap();
    assert!(
        entry.repo.is_some(),
        "fixture must keep its repo for this to be meaningful"
    );
    entry.repo_source = None;
    report
}

#[test]
fn predates_fidelity_fields_true_when_repo_is_present_without_any_repo_source() {
    assert!(predates_fidelity_fields(&pre_change_report()));
}

#[test]
fn predates_fidelity_fields_false_on_a_current_report() {
    assert!(!predates_fidelity_fields(&sample_report()));
}

#[test]
fn predates_fidelity_fields_false_when_no_session_has_a_repo_at_all() {
    let mut report = sample_report();
    for entry in report.sessions.values_mut() {
        entry.repo = None;
        entry.repo_source = None;
    }
    assert!(
        !predates_fidelity_fields(&report),
        "no session claims a repo at all, so there is nothing to have predated"
    );
}

/// `--prior` lights up the `prior` context key: figures are COPIED from the prior artifact via the
/// same `build_totals_view`/`aggregate::compute` the current period uses, never recomputed from a
/// diff against the current period.
#[test]
fn build_context_block_includes_prior_when_supplied() {
    let tmp = TempDir::new().unwrap();
    let prior_report = sample_report(); // since=2026-04-01 until=2026-04-30 -> 30 days inclusive
    let prior_path = write_report_json(&tmp, "prior.json", &prior_report);

    let mut current = sample_report();
    current.since = ts("2026-05-01T00:00:00Z");
    current.until = ts("2026-05-30T00:00:00Z"); // also 30 days, so the two periods are comparable

    let block = build_context_block(
        &current,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        Some(&prior_path),
        None,
        None,
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let prior = parsed
        .get("prior")
        .expect("prior key must be present when --prior is supplied");

    assert_eq!(prior.get("since").and_then(|v| v.as_str()), Some("2026-04-01"));
    assert_eq!(prior.get("until").and_then(|v| v.as_str()), Some("2026-04-30"));
    assert_eq!(prior.get("days").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(prior.get("comparable").and_then(|v| v.as_bool()), Some(true));
    assert!(
        prior.get("predates-fields").is_none(),
        "a current-shaped artifact carries no predates-fields caveat"
    );

    let totals = prior.get("totals").expect("prior.totals key");
    assert_eq!(totals.get("spend").and_then(|v| v.as_str()), Some("$0.60"));
    assert_eq!(totals.get("sessions").and_then(|v| v.as_u64()), Some(2));

    let by_repo = prior
        .get("by-repo")
        .and_then(|v| v.as_array())
        .expect("prior.by-repo array");
    assert!(
        by_repo
            .iter()
            .any(|r| r.get("repo").and_then(|v| v.as_str()) == Some("tatari-tv/claude-report")),
        "prior.by-repo must carry the repo the prior artifact attributed spend to: {by_repo:?}"
    );
    assert!(
        prior.get("by-org").and_then(|v| v.as_array()).is_some(),
        "prior.by-org must be present"
    );
}

/// Without `--prior`, the `prior` key is entirely absent (never an empty object), so the prompt's
/// "omit the section" rule needs no special-case for a present-but-empty block.
#[test]
fn build_context_block_omits_prior_key_without_the_flag() {
    let report = sample_report();
    let block = build_context_block(
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
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    assert!(parsed.get("prior").is_none(), "no --prior -> no prior key at all");
}

/// A prior window whose length differs from the current period's sets `comparable: false`, so the
/// prompt states the length mismatch rather than reading a 14-day prior against a 30-day current
/// period as if they covered equal ground.
#[test]
fn build_context_block_prior_comparable_is_false_on_a_length_mismatch() {
    let tmp = TempDir::new().unwrap();
    let mut prior_report = sample_report();
    prior_report.since = ts("2026-04-01T00:00:00Z");
    prior_report.until = ts("2026-04-14T00:00:00Z"); // 14 days, not 30
    let prior_path = write_report_json(&tmp, "prior.json", &prior_report);

    let current = sample_report(); // 2026-04-01..2026-04-30 = 30 days
    let block = build_context_block(
        &current,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        Some(&prior_path),
        None,
        None,
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let prior = parsed.get("prior").expect("prior key");
    assert_eq!(prior.get("days").and_then(|v| v.as_i64()), Some(14));
    assert_eq!(
        prior.get("comparable").and_then(|v| v.as_bool()),
        Some(false),
        "a 14-day prior against a 30-day current period must not read as comparable"
    );
}

/// A pre-change prior artifact (repo present, no repo-source anywhere) states the
/// predates-the-fields caveat and omits `prior.outcomes` entirely, even when the artifact's own
/// `lines-written`/`lines-replaced` are exactly zero -- the case a naive implementation would
/// render as a real zero measurement instead of "not measured".
#[test]
fn build_context_block_prior_states_predates_fields_instead_of_zeros() {
    let tmp = TempDir::new().unwrap();
    let mut prior_report = pre_change_report();
    prior_report.totals.outcomes = Some(crate::outcome::OutcomeTotals {
        sessions_with_commits: 1,
        commits: 3,
        prs_opened: 1,
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 5,
        lines_written: 0,
        lines_replaced: 0,
    });
    let prior_path = write_report_json(&tmp, "prior.json", &prior_report);

    let current = sample_report();
    let block = build_context_block(
        &current,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        Some(&prior_path),
        None,
        None,
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let prior = parsed.get("prior").expect("prior key");

    let note = prior
        .get("predates-fields")
        .and_then(|v| v.as_str())
        .expect("predates-fields caveat must be present for a pre-change artifact");
    assert!(
        note.contains("repo-source"),
        "the caveat must name what the artifact predates: {note}"
    );
    assert!(
        prior.get("outcomes").is_none(),
        "a pre-change prior must never render its outcomes as measurements: {prior}"
    );
    // totals/by-repo/by-org are legitimate even on a pre-change artifact (spend and session counts
    // predate every fidelity fix this design adds), so they still carry real figures.
    assert_eq!(
        prior
            .get("totals")
            .and_then(|t| t.get("spend"))
            .and_then(|v| v.as_str()),
        Some("$0.60")
    );
}

/// `--prior` is schema-gated exactly like `-i`: a v1 (or any non-matching) artifact fails loudly,
/// naming both versions and the remedy, rather than surfacing a raw serde error.
#[test]
fn build_context_block_bails_on_a_wrong_schema_prior() {
    let tmp = TempDir::new().unwrap();
    let prior_path = tmp.path().join("prior.json");
    fs::write(&prior_path, r#"{"schema-version":1}"#).unwrap();

    let current = sample_report();
    let err = build_context_block(
        &current,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        Some(&prior_path),
        None,
        None,
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("schema v1") && msg.contains("report collect"),
        "the --prior schema gate must name the mismatch and the remedy: {msg}"
    );
}

/// A `--prior` path that does not exist fails loudly, naming the flag so the operator does not
/// mistake it for the primary `-i` input failing.
#[test]
fn build_context_block_bails_when_prior_path_does_not_exist() {
    let current = sample_report();
    let missing = std::path::PathBuf::from("/nonexistent/prior.json");
    let err = build_context_block(
        &current,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        Some(&missing),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("--prior"),
        "the read failure must name --prior, not read as the primary input failing"
    );
}
